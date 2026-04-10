use std::collections::{HashMap, HashSet, VecDeque};

use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

use crate::routing;

pub type NodeId = usize;

/// Number of distinct virus strains. Used by strain-indexed palettes in
/// render and by the modular wraparound in spread-tally logic.
pub const STRAIN_COUNT: usize = 8;

/// Worms advance one cell every N sim ticks so each cell stays on-screen
/// long enough to see.
const WORM_STEP_INTERVAL: u64 = 2;

/// Link load threshold for the "warm" render tier: accent color with a
/// bold modifier. Below this the link uses its normal branch hue.
pub const WARM_LINK: u8 = 6;
/// Link load threshold for the "hot" render tier: cascade color. Packets
/// refuse to hop onto a link whose load has crossed this.
pub const HOT_LINK: u8 = 16;
/// How much each in-flight packet adds to its current link's load per tick.
const PACKET_LOAD_INCREMENT: u8 = 2;
/// How much each in-flight worm adds to its current link's load per tick.
const WORM_LOAD_INCREMENT: u8 = 1;

/// Duration in ticks of a scanner's ping pulse. Adjacent links brighten
/// to the scanner color for this many ticks — no strobe, no reversed
/// fill, just a quiet lift over the branch hue.
const SCANNER_PULSE_TICKS: u8 = 8;

/// How many ticks an exploit-chain breach mark stays on a link before
/// decaying. The chain walks from the pwned node toward C2 and all
/// traversed links glow for this many ticks, telling the story of
/// where the attack came from.
const BREACH_TTL: u8 = 12;
/// Maximum hops to walk up the parent chain when marking a breach.
/// Caps both the work done and the visual length of the breach tail.
const BREACH_MAX_HOPS: usize = 10;

/// Zero-day event weights. Rolls `0.0..1.0`: outbreak below the first
/// threshold, emergency patch below the second, immune breakthrough above.
const ZERO_DAY_OUTBREAK_WEIGHT: f32 = 0.6;
const ZERO_DAY_PATCH_WEIGHT: f32 = 0.9;
const ZERO_DAY_OUTBREAK_MIN: u32 = 3;
const ZERO_DAY_OUTBREAK_MAX: u32 = 5;
const ZERO_DAY_MIN_ALIVE: usize = 10;

#[derive(Clone, Copy, Debug)]
pub enum State {
    Alive,
    Pwned { ticks_left: u8 },
    Dead,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Relay,
    Scanner,
    Exfil,
    Honeypot,
    /// Patrols its neighborhood and applies a local cure pulse to nearby
    /// infected nodes. Immune to infection itself; never mutates.
    Defender,
    /// Fortified core node with extra pwn-absorbing charges. Spawns only
    /// close to its faction's C2, creating visible fortified zones.
    Tower,
    /// Rally-point node that boosts nearby nodes' parent-selection weight,
    /// creating visible spawn clusters. Rendered with an always-on glow.
    Beacon,
    /// Scanner repeater — when any scanner within `proxy_radius` fires,
    /// this node also pulses, propagating the scanner highlight through
    /// a chain of proxies.
    Proxy,
    /// Looks like an exfil but never emits packets — a passive
    /// camouflage node that draws attacker attention away from real
    /// exfils. Rendered identically to an Exfil.
    Decoy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InfectionStage {
    /// No visible symptoms yet, but still spreads.
    Incubating,
    /// Flickering glyph, normal role behavior suppressed.
    Active,
    /// About to crash the host — counts down `terminal_ticks` then forces a pwn.
    Terminal,
}

#[derive(Clone, Copy, Debug)]
pub struct Infection {
    pub strain: u8,
    pub stage: InfectionStage,
    pub age: u16,
    /// Decremented by patch waves (commit 2); at 0 the infection is cured.
    #[allow(dead_code)]
    pub cure_resist: u8,
    pub terminal_ticks: u8,
}

impl Infection {
    pub fn seeded(strain: u8, cure_resist: u8) -> Self {
        Self {
            strain,
            stage: InfectionStage::Incubating,
            age: 0,
            cure_resist,
            terminal_ticks: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Node {
    pub pos: (i16, i16),
    pub parent: Option<NodeId>,
    pub state: State,
    pub born: u64,
    pub pulse: u8,
    /// Nonzero means scheduled to die; render as red ✕ until it hits 0, then
    /// flip to Dead. Set via schedule_subtree_death with a delay proportional
    /// to distance from the pwned root, producing a visible red ripple through
    /// the subtree.
    pub dying_in: u8,
    pub role: Role,
    pub hardened: bool,
    pub heartbeats: u8,
    pub branch_id: u16,
    pub role_cooldown: u16,
    pub last_ping_dir: Option<(i8, i8)>,
    pub last_ping_tick: u64,
    pub honey_tripped: bool,
    pub honey_reveal: u8,
    /// Nonzero means a pwn attempt was just absorbed; renders as a bright
    /// shield glyph for a few ticks so the viewer sees the save happen.
    pub shield_flash: u8,
    pub infection: Option<Infection>,
    /// Nonzero means the node just mutated its role — flashes pink.
    pub mutated_flash: u8,
    /// Nonzero while a Scanner's ping pulse is active. The render pass
    /// uses this to brighten the scanner node itself and every link
    /// adjacent to it, creating a visible "surveying" pulse that rolls
    /// through the local topology instead of phantom dots in empty space.
    pub scan_pulse: u8,
    /// Extra pwn-absorbing charges beyond the normal heartbeat-driven
    /// shield. Towers spawn with a nonzero value; each pwn attempt
    /// decrements this counter before touching the regular hardened
    /// flag. Independent from `hardened` so towers can still gain and
    /// spend a heartbeat shield on top of their fortification.
    pub pwn_resist: u8,
    /// Which C2 this node belongs to (index into `World.c2_nodes`).
    /// Inherited from parent at spawn; first-hop C2 children take their
    /// C2's index. Used to keep cascade reachability and cross-link
    /// reconnects faction-isolated.
    pub faction: u8,
}

impl Node {
    /// Active or Terminal infection suppresses this node's role behaviors
    /// (scanner pings, exfil packets). Incubating infections remain stealthy.
    pub fn role_suppressed(&self) -> bool {
        matches!(
            &self.infection,
            Some(i) if !matches!(i.stage, InfectionStage::Incubating)
        )
    }

    pub fn fresh(pos: (i16, i16), parent: Option<NodeId>, born: u64, role: Role, branch_id: u16) -> Self {
        Self {
            pos,
            parent,
            state: State::Alive,
            born,
            pulse: 0,
            dying_in: 0,
            role,
            hardened: false,
            heartbeats: 0,
            branch_id,
            role_cooldown: 0,
            last_ping_dir: None,
            last_ping_tick: 0,
            honey_tripped: false,
            honey_reveal: 0,
            shield_flash: 0,
            infection: None,
            mutated_flash: 0,
            scan_pulse: 0,
            pwn_resist: 0,
            faction: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    /// Tree edge created when a node is spawned from its parent.
    Parent,
    /// Lateral bridge between two live nodes in different branches. Used
    /// purely for cascade reachability — packets never relay through these.
    Cross,
}

#[derive(Clone, Debug)]
pub struct Link {
    pub a: NodeId,
    pub b: NodeId,
    pub path: Vec<(i16, i16)>,
    pub drawn: u16,
    pub kind: LinkKind,
    /// Accumulated traffic load. Each in-flight packet adds +2 per tick,
    /// each worm +1. Decays by 1 per tick. The renderer blends into
    /// hotter colors as load crosses WARM_LINK and HOT_LINK thresholds;
    /// packets refuse to hop onto a link whose load is above HOT_LINK.
    pub load: u8,
    /// Nonzero while this link is part of a recent exploit chain. Set
    /// when a pwn event walks up the parent chain from the victim
    /// toward C2; decays each tick in decay_link_load.
    pub breach_ttl: u8,
}

#[derive(Clone, Debug)]
pub struct Packet {
    pub link_id: usize,
    /// Index into link.path. Packets travel from the child end (high index)
    /// toward the parent end (index 0).
    pub pos: u16,
}

#[derive(Clone, Debug)]
pub struct Worm {
    pub link_id: usize,
    pub pos: u16,
    /// True if the worm started at `link.a` and is traveling toward `link.b`;
    /// false for the reverse. Cross-links are bidirectional so both are valid.
    pub outbound_from_a: bool,
    pub strain: u8,
}

/// Transient particle ejected from a cascade root. Sub-cell f32
/// position + velocity so multiple sparks in one terminal cell can
/// render as distinct braille dots.
#[derive(Clone, Debug)]
pub struct CascadeSpark {
    pub pos: (f32, f32),
    pub vel: (f32, f32),
    pub age: u8,
    pub life: u8,
}

/// Expanding shockwave ring drawn at a cascade root. Age increments
/// once per tick; radius tracks age directly. Cells on the ring get
/// rendered as bold braille chars.
#[derive(Clone, Debug)]
pub struct CascadeShockwave {
    pub origin: (i16, i16),
    pub age: u8,
    pub max_age: u8,
}

#[derive(Clone, Debug)]
pub struct PatchWave {
    pub origin: (i16, i16),
    pub radius: i16,
}

#[derive(Clone, Debug)]
pub struct RoleWeights {
    pub relay: f32,
    pub scanner: f32,
    pub exfil: f32,
    pub honeypot: f32,
    pub defender: f32,
    pub tower: f32,
    pub beacon: f32,
    pub proxy: f32,
    pub decoy: f32,
}

impl Default for RoleWeights {
    fn default() -> Self {
        Self {
            relay: 0.51,
            scanner: 0.13,
            exfil: 0.10,
            honeypot: 0.04,
            defender: 0.08,
            tower: 0.05,
            beacon: 0.04,
            proxy: 0.03,
            decoy: 0.02,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub p_spawn: f32,
    pub p_loss: f32,
    pub base_dist: i16,
    pub max_nodes: usize,
    pub heartbeat_period: u64,
    pub pwned_flash_ticks: u8,
    pub log_cap: usize,
    pub role_weights: RoleWeights,
    pub scanner_ping_period: u16,
    pub exfil_packet_period: u16,
    pub hardened_after_heartbeats: u8,
    pub honeypot_cascade_mult: f32,
    pub reconnect_rate: f32,
    pub reconnect_radius: i16,
    pub virus_spread_rate: f32,
    pub virus_incubation_ticks: u16,
    pub virus_active_ticks: u16,
    pub virus_terminal_ticks: u8,
    pub virus_cure_resist: u8,
    pub virus_seed_rate: f32,
    pub worm_spawn_rate: f32,
    pub patch_wave_radius: i16,
    pub mutate_rate: f32,
    pub mutate_min_age: u64,
    pub zero_day_period: u64,
    pub zero_day_chance: f32,
    /// Constant weight given to C2 in the parent-selection roll. Without
    /// this, C2's age-decayed weight collapses below all frontier nodes
    /// after the first few ticks and the mesh stops minting new branches.
    pub c2_spawn_bias: f32,
    /// Per-spawn probability that a new node starts its own branch instead
    /// of inheriting its parent's branch_id. Lets distinct sub-botnets fork
    /// off existing nodes anywhere in the mesh, not just at C2.
    pub fork_rate: f32,
    /// Ticks between defender cure pulses.
    pub defender_pulse_period: u16,
    /// Chebyshev radius of a defender's local cure pulse.
    pub defender_radius: i16,
    /// Number of C2 nodes / factions to spawn at the start of the run.
    /// 1 = classic single botnet; 2+ = competing factions.
    pub c2_count: u8,
    /// Length of a full day/night cycle in ticks. Spawn and loss rates
    /// oscillate across this period, creating visible waves of activity.
    /// 0 disables the effect entirely.
    pub day_night_period: u64,
    /// Multiplier applied to p_spawn during the night half of the cycle.
    pub night_spawn_mult: f32,
    /// Multiplier applied to p_loss during the night half of the cycle.
    pub night_loss_mult: f32,
    /// Chebyshev radius searched for honeypot backdoor targets. When a
    /// honeypot trips, it reveals up to `honeypot_backdoor_max` new
    /// cross-links to nearby same-faction / different-branch neighbors
    /// before cascading.
    pub honeypot_backdoor_radius: i16,
    /// Maximum number of backdoor cross-links a single honeypot trip
    /// may reveal.
    pub honeypot_backdoor_max: u8,
    /// Ticks between network-storm rolls. A storm is a rare event that
    /// briefly spikes both spawn and loss, producing a chaotic burst.
    /// Set to 0 to disable.
    pub storm_period: u64,
    /// Probability of a storm firing when `storm_period` elapses.
    pub storm_chance: f32,
    /// How many ticks a storm stays active once it fires.
    pub storm_duration: u64,
    /// Multiplier applied to p_spawn while a storm is active.
    pub storm_spawn_mult: f32,
    /// Multiplier applied to p_loss while a storm is active.
    pub storm_loss_mult: f32,
    /// Maximum Chebyshev distance from any C2 at which a Tower may
    /// spawn. Spawns rolling a Tower role beyond this distance fall
    /// back to Relay, so fortified cores stay near their faction hub.
    pub tower_spawn_radius: i16,
    /// Extra pwn-absorbing charges a newly spawned Tower receives.
    pub tower_pwn_resist: u8,
    /// Length of a single named epoch in ticks. Each time the sim crosses
    /// a multiple of this value, it enters a new era with a name drawn
    /// from ERA_NAMES. Set to 0 to disable.
    pub epoch_period: u64,
    /// Radius within which a Proxy node echoes a firing scanner's
    /// pulse. When a scanner fires, every proxy inside this Chebyshev
    /// radius also gets scan_pulse set, so the pulse ripples through
    /// a chain of proxies.
    pub proxy_radius: i16,
    /// Radius within which an alive Beacon boosts a candidate's
    /// parent-selection weight during try_spawn.
    pub beacon_radius: i16,
    /// Multiplier added to a candidate's parent weight per nearby beacon.
    /// Default 1.5x means being next to a beacon roughly 2.5x a node's
    /// chance of being chosen to spawn the next child.
    pub beacon_weight_mult: f32,
    /// A cascade of this size or larger logs 'THE BIG ONE' as a mythic
    /// event. Tune lower for a smaller mesh or to see it more often.
    pub mythic_big_one_threshold: usize,
    /// If greater than `c2_count`, World::new rolls a random starting
    /// C2 count in `c2_count..=c2_count_max` instead of the fixed
    /// value. Lets each seed produce a differently-shaped opening.
    pub c2_count_max: u8,
    /// Minimum size of a cascade batch that can trigger a rebirth
    /// roll. Below this, cascades never resurrect anything.
    pub resurrection_threshold: u8,
    /// Chance that a qualifying cascade batch births a new C2 from
    /// one of its ashes.
    pub resurrection_chance: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            p_spawn: 0.15,
            p_loss: 0.005,
            base_dist: 4,
            max_nodes: 400,
            heartbeat_period: 20,
            pwned_flash_ticks: 6,
            log_cap: 32,
            role_weights: RoleWeights::default(),
            scanner_ping_period: 30,
            exfil_packet_period: 25,
            hardened_after_heartbeats: 10,
            honeypot_cascade_mult: 3.0,
            reconnect_rate: 0.0,
            reconnect_radius: 10,
            virus_spread_rate: 0.05,
            virus_incubation_ticks: 30,
            virus_active_ticks: 80,
            virus_terminal_ticks: 20,
            // With the width-1 patch wave (post-bugfix) each wave decrements
            // cure_resist exactly once per pass. Set to 2 so infections clear
            // after two heartbeat sweeps, matching the pre-fix feel.
            virus_cure_resist: 2,
            virus_seed_rate: 0.004,
            worm_spawn_rate: 0.15,
            patch_wave_radius: 10,
            mutate_rate: 0.0008,
            mutate_min_age: 400,
            zero_day_period: 2000,
            zero_day_chance: 0.4,
            c2_spawn_bias: 0.6,
            fork_rate: 0.05,
            defender_pulse_period: 25,
            defender_radius: 5,
            // Default 1 keeps single-faction tests and library callers
            // simple. The CLI defaults to 2 so the released binary feels
            // like factions are "on".
            c2_count: 1,
            day_night_period: 600,
            night_spawn_mult: 1.6,
            night_loss_mult: 1.5,
            honeypot_backdoor_radius: 14,
            honeypot_backdoor_max: 3,
            storm_period: 1800,
            storm_chance: 0.35,
            storm_duration: 150,
            storm_spawn_mult: 2.2,
            storm_loss_mult: 2.2,
            tower_spawn_radius: 10,
            tower_pwn_resist: 2,
            proxy_radius: 8,
            beacon_radius: 6,
            beacon_weight_mult: 1.5,
            epoch_period: 5000,
            mythic_big_one_threshold: 30,
            c2_count_max: 0,
            resurrection_threshold: 12,
            resurrection_chance: 0.35,
        }
    }
}

/// Pool of ominous names the sim draws from when assigning display
/// names to its STRAIN_COUNT virus strains at world start. Every run
/// picks 8 distinct names from this pool so the strains feel like
/// named threats instead of numbered enumerants.
pub const STRAIN_NAME_POOL: &[&str] = &[
    "Cerberus",
    "Hydra",
    "Phantom",
    "Wraith",
    "Basilisk",
    "Cobra",
    "Kraken",
    "Chimera",
    "Gorgon",
    "Banshee",
    "Lich",
    "Nyx",
    "Eris",
    "Hecate",
    "Tartarus",
    "Styx",
    "Omen",
    "Pandora",
    "Morrigan",
    "Azrael",
];

/// Named eras the sim cycles through as it ages. Fully ephemeral — no
/// gameplay effect, just a long-arc narrative marker in the header and
/// the log so long sessions feel like they're going somewhere.
pub const ERA_NAMES: &[&str] = &[
    "Age of Silence",
    "First Signal",
    "Rise of the Mesh",
    "Era of Cascades",
    "Winter of Quarantine",
    "The Great Spreading",
    "Dusk Protocols",
    "Zero-Day Bloom",
    "Age of Wires",
    "Final Handshake",
    "Echo Chamber",
    "The Long Drift",
];

pub struct World {
    pub nodes: Vec<Node>,
    pub links: Vec<Link>,
    /// Indices into `nodes` of every C2 node. Each is the root of its own
    /// faction; the first entry doubles as the "primary" C2 used by code
    /// that only needs a single reference (tests, render conveniences).
    pub c2_nodes: Vec<NodeId>,
    pub rng: ChaCha8Rng,
    pub tick: u64,
    pub occupied: HashSet<(i16, i16)>,
    /// Ring buffer of log lines paired with a repeat counter. When
    /// `push_log` receives the same message as the most recent entry,
    /// it increments the counter instead of appending a duplicate, so
    /// chatty events collapse to 'node X.Y hardened (×3)' in the UI.
    pub logs: VecDeque<(String, u32)>,
    pub bounds: (i16, i16),
    pub cfg: Config,
    pub packets: Vec<Packet>,
    pub worms: Vec<Worm>,
    pub patch_waves: Vec<PatchWave>,
    pub sparks: Vec<CascadeSpark>,
    pub shockwaves: Vec<CascadeShockwave>,
    pub next_branch_id: u16,
    /// Tick at which the current network storm ends. 0 if no storm is
    /// active. Storms spike both spawn and loss rates for a short burst.
    pub storm_until: u64,
    /// Display name per strain id. Selected once at World::new from
    /// STRAIN_NAME_POOL using the seeded RNG, so a fixed seed always
    /// produces the same strain identities.
    pub strain_names: [&'static str; STRAIN_COUNT],
    /// Per-faction running stats. Indexed by faction id and sized to
    /// c2_count at World::new.
    pub faction_stats: Vec<FactionStats>,
    /// True once the 'PANDEMIC' mythic event has fired this run. Used
    /// to make sure it only lands once even if the condition persists.
    pub mythic_pandemic_seen: bool,
    /// Rolling window of total alive-node counts sampled on the same
    /// cadence as faction history. Feeds the btop-style braille area
    /// graph in the right column's 'activity' panel.
    pub activity_history: VecDeque<u32>,
}

impl World {
    /// The primary C2 — the first one spawned, used by single-faction code
    /// paths and tests. Always exists because c2_nodes is non-empty.
    #[allow(dead_code)]
    pub fn c2(&self) -> NodeId {
        self.c2_nodes[0]
    }

    pub fn is_c2(&self, id: NodeId) -> bool {
        self.c2_nodes.contains(&id)
    }

    /// Push one sample of each faction's alive-node count into its
    /// history ring, plus one sample of the total alive count into
    /// the activity history window.
    fn sample_faction_history(&mut self) {
        let mut counts = vec![0u32; self.faction_stats.len()];
        let mut total: u32 = 0;
        for n in &self.nodes {
            if matches!(n.state, State::Alive) {
                total += 1;
                if let Some(slot) = counts.get_mut(n.faction as usize) {
                    *slot += 1;
                }
            }
        }
        for (stats, count) in self.faction_stats.iter_mut().zip(counts.into_iter()) {
            stats.history.push_back(count);
            while stats.history.len() > FACTION_HISTORY_LEN {
                stats.history.pop_front();
            }
        }
        self.activity_history.push_back(total);
        while self.activity_history.len() > ACTIVITY_HISTORY_LEN {
            self.activity_history.pop_front();
        }
    }

    /// True during the night half of the day/night cycle. When the period
    /// is zero the cycle is disabled and this always returns false.
    pub fn is_night(&self) -> bool {
        let period = self.cfg.day_night_period;
        if period == 0 {
            return false;
        }
        (self.tick % period) >= period / 2
    }

    /// True while a network storm is currently active.
    pub fn is_storming(&self) -> bool {
        self.storm_until > self.tick
    }

    /// Index of the current named era, 0-based. Returns 0 when epoch
    /// tracking is disabled.
    pub fn epoch_index(&self) -> usize {
        let period = self.cfg.epoch_period;
        if period == 0 {
            return 0;
        }
        (self.tick / period) as usize
    }

    /// Name of the current era, cycling through ERA_NAMES.
    pub fn epoch_name(&self) -> &'static str {
        ERA_NAMES[self.epoch_index() % ERA_NAMES.len()]
    }
}

const DIRS: [(i16, i16); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

/// Running tally of notable events per faction. Used for the header
/// prestige readout and the end-of-run summary.
#[derive(Clone, Debug, Default)]
pub struct FactionStats {
    pub spawned: u32,
    pub lost: u32,
    pub honeys_tripped: u32,
    pub infections_cured: u32,
    /// Recent alive-node count samples, bounded to FACTION_HISTORY_LEN.
    /// Sampled on a slow cadence so the header sparkline reads as a
    /// smooth trend rather than a jittering count.
    pub history: std::collections::VecDeque<u32>,
}

/// Number of samples kept in each faction's alive-count history.
pub const FACTION_HISTORY_LEN: usize = 8;
/// Number of samples kept in the global activity history window.
/// Larger than per-faction because the activity panel is a wider
/// braille graph.
pub const ACTIVITY_HISTORY_LEN: usize = 64;
/// Tick interval between FactionStats.history samples.
const FACTION_SAMPLE_PERIOD: u64 = 50;

impl FactionStats {
    /// Composite "prestige" score. Positive for growth, negative for
    /// churn. Honeypot traps and cures are rewarded so defense-
    /// leaning factions can still score well without hoarding nodes.
    pub fn score(&self) -> i32 {
        self.spawned as i32 - 3 * (self.lost as i32)
            + 5 * (self.honeys_tripped as i32)
            + 2 * (self.infections_cured as i32)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WorldStats {
    pub alive: usize,
    pub pwned: usize,
    pub dead: usize,
    pub dying: usize,
    pub branches: usize,
    pub factions: usize,
    pub links: usize,
    pub cross_links: usize,
    pub packets: usize,
    pub infected: usize,
}

impl World {
    pub fn stats(&self) -> WorldStats {
        let mut s = WorldStats::default();
        let mut branches: HashSet<u16> = HashSet::new();
        for n in &self.nodes {
            match n.state {
                State::Alive => s.alive += 1,
                State::Pwned { .. } => s.pwned += 1,
                State::Dead => s.dead += 1,
            }
            if n.dying_in > 0 {
                s.dying += 1;
            }
            if !matches!(n.state, State::Dead) {
                branches.insert(n.branch_id);
            }
            if n.infection.is_some() && !matches!(n.state, State::Dead) {
                s.infected += 1;
            }
        }
        s.branches = branches.len();
        s.factions = self
            .c2_nodes
            .iter()
            .filter(|&&id| !matches!(self.nodes[id].state, State::Dead))
            .count();
        s.links = self.links.len();
        s.cross_links = self
            .links
            .iter()
            .filter(|l| l.kind == LinkKind::Cross)
            .count();
        s.packets = self.packets.len();
        s
    }

    pub fn new(seed: u64, bounds: (i16, i16), cfg: Config) -> Self {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        // Pick STRAIN_COUNT distinct names from the pool. Done up front
        // so the rest of the constructor can consume the same rng.
        let strain_names = {
            let mut pool: Vec<&'static str> = STRAIN_NAME_POOL.to_vec();
            pool.shuffle(&mut rng);
            let mut arr: [&'static str; STRAIN_COUNT] = ["?"; STRAIN_COUNT];
            for (slot, name) in arr.iter_mut().zip(pool.into_iter()) {
                *slot = name;
            }
            arr
        };
        // Randomize the opening C2 count if the config asks for it.
        let min = cfg.c2_count.max(1);
        let max = cfg.c2_count_max.max(min);
        let count = if max > min {
            rng.gen_range(min..=max) as usize
        } else {
            min as usize
        };
        let mut nodes: Vec<Node> = Vec::with_capacity(count);
        let mut occupied = HashSet::new();
        let mut logs: VecDeque<(String, u32)> = VecDeque::new();
        let mut c2_nodes: Vec<NodeId> = Vec::with_capacity(count);

        // Random placement with edge margin + minimum spacing between
        // C2s, so no two C2s land directly on top of each other and
        // none stick to a wall.
        let margin_x = ((bounds.0 / 10).max(4)).min(bounds.0 / 2 - 1);
        let margin_y = ((bounds.1 / 6).max(3)).min(bounds.1 / 2 - 1);
        let min_spacing = (bounds.0.min(bounds.1) / 4).max(10);

        for i in 0..count {
            let mut chosen: Option<(i16, i16)> = None;
            for _ in 0..64 {
                let x = rng.gen_range(margin_x..bounds.0 - margin_x);
                let y = rng.gen_range(margin_y..bounds.1 - margin_y);
                let cand = (x, y);
                let too_close = c2_nodes.iter().any(|&id| {
                    let p: (i16, i16) = nodes[id].pos;
                    (p.0 - cand.0).abs().max((p.1 - cand.1).abs()) < min_spacing
                });
                if !too_close {
                    chosen = Some(cand);
                    break;
                }
            }
            // Fallback: if random placement can't find a free slot
            // within the spacing budget, fall back to evenly-spaced
            // slots on the midline so the world still constructs.
            let pos = chosen.unwrap_or_else(|| {
                if count == 1 {
                    (bounds.0 / 2, bounds.1 / 2)
                } else {
                    let denom = (count + 1) as i16;
                    let x = bounds.0 * (i as i16 + 1) / denom;
                    (x, bounds.1 / 2)
                }
            });
            let mut node = Node::fresh(pos, None, 0, Role::Relay, 0);
            node.faction = i as u8;
            let id = nodes.len();
            nodes.push(node);
            occupied.insert(pos);
            c2_nodes.push(id);
            logs.push_back((format!("c2[{}] online @ {},{}", i, pos.0, pos.1), 1));
        }

        Self {
            nodes,
            links: Vec::new(),
            c2_nodes,
            rng,
            tick: 0,
            occupied,
            logs,
            bounds,
            cfg,
            packets: Vec::new(),
            worms: Vec::new(),
            patch_waves: Vec::new(),
            sparks: Vec::new(),
            shockwaves: Vec::new(),
            next_branch_id: 1,
            storm_until: 0,
            strain_names,
            faction_stats: vec![FactionStats::default(); count],
            mythic_pandemic_seen: false,
            activity_history: VecDeque::with_capacity(ACTIVITY_HISTORY_LEN),
        }
    }

    /// Display name for a strain id, wrapping into the name pool if the
    /// id is out of bounds.
    pub fn strain_name(&self, strain: u8) -> &'static str {
        self.strain_names[(strain as usize) % STRAIN_COUNT]
    }

    pub fn tick(&mut self, bounds: (i16, i16)) {
        self.bounds = bounds;

        // Day/night transition detection. Log the change before the tick
        // so operators can see the phase swap lined up with the new events.
        let period = self.cfg.day_night_period;
        if period > 0 && self.tick > 0 {
            let prev = self.tick.saturating_sub(1) % period >= period / 2;
            let curr = self.tick % period >= period / 2;
            if prev != curr {
                let msg = if curr {
                    "night falls — activity spikes"
                } else {
                    "day breaks — mesh settles"
                };
                self.push_log(msg.to_string());
            }
        }

        // Epoch transition: crossing a multiple of epoch_period enters
        // a new named era. Pure flavor — no gameplay effect.
        let epoch_period = self.cfg.epoch_period;
        if epoch_period > 0 && self.tick > 0 && self.tick.is_multiple_of(epoch_period) {
            let idx = (self.tick / epoch_period) as usize;
            let name = ERA_NAMES[idx % ERA_NAMES.len()];
            self.push_log(format!("── era {}: {}", idx, name));
        }

        // Network storm: rare chaotic burst that spikes spawn + loss for
        // a short window. Logged at start and end so the phase reads
        // clearly in the log.
        self.maybe_storm();

        // Sample faction alive counts for the header sparkline.
        if self.tick.is_multiple_of(FACTION_SAMPLE_PERIOD) {
            self.sample_faction_history();
        }

        // Phase 1: growth — add new nodes and extend link animations.
        self.try_spawn();
        self.advance_links();

        // Phase 2: traveler motion — anything moving along existing links.
        self.decay_link_load();
        self.advance_packets();
        self.advance_worms();
        self.advance_patch_waves();
        self.advance_sparks();
        self.advance_shockwaves();

        // Phase 3: periodic sweeps + per-node upkeep.
        self.heartbeat();
        self.advance_role_cooldowns();
        self.maybe_mutate();
        self.maybe_zero_day();

        // Phase 4: role-driven emissions. Must run after cooldowns so the
        // period timers have already been decremented for this tick.
        self.fire_scanner_pings();
        self.fire_exfil_packets();
        self.fire_defender_pulses();

        // Phase 5: infection dynamics — stage progression, spread, seeding,
        // and worm launches from active carriers.
        self.advance_infections();
        self.maybe_spawn_worms();
        self.maybe_seed_infection();

        // Phase 6: loss, cascade, and mesh repair.
        self.advance_pwned_and_loss();
        self.advance_dying();
        self.maybe_reconnect();

        self.tick += 1;
    }

    fn maybe_reconnect(&mut self) {
        if self.cfg.reconnect_rate <= 0.0 {
            return;
        }
        if !self.rng.gen_bool(self.cfg.reconnect_rate.clamp(0.0, 1.0) as f64) {
            return;
        }
        let alive: Vec<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                if !self.is_c2(i)
                    && matches!(n.state, State::Alive)
                    && n.dying_in == 0
                {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();
        if alive.len() < 2 {
            return;
        }
        let a = alive[self.rng.gen_range(0..alive.len())];
        let a_pos = self.nodes[a].pos;
        let a_branch = self.nodes[a].branch_id;
        let a_faction = self.nodes[a].faction;
        let radius = self.cfg.reconnect_radius;
        let mut candidates: Vec<NodeId> = alive
            .iter()
            .copied()
            .filter(|&b| {
                if b == a {
                    return false;
                }
                if self.nodes[b].branch_id == a_branch {
                    return false;
                }
                // Cross-links stay within faction so cascades and reachability
                // remain faction-isolated.
                if self.nodes[b].faction != a_faction {
                    return false;
                }
                let dp = self.nodes[b].pos;
                (dp.0 - a_pos.0).abs().max((dp.1 - a_pos.1).abs()) <= radius
            })
            .collect();
        if candidates.is_empty() {
            return;
        }
        // Skip if a cross link already exists between these two.
        let already_linked = |x: NodeId, y: NodeId, links: &[Link]| {
            links.iter().any(|l| {
                l.kind == LinkKind::Cross
                    && ((l.a == x && l.b == y) || (l.a == y && l.b == x))
            })
        };
        candidates.retain(|&b| !already_linked(a, b, &self.links));
        if candidates.is_empty() {
            return;
        }
        let b = candidates[self.rng.gen_range(0..candidates.len())];
        let b_pos = self.nodes[b].pos;
        let path = match routing::route_link(
            a_pos,
            b_pos,
            &self.occupied,
            self.bounds,
            &mut self.rng,
        ) {
            Some(p) => p,
            None => return,
        };
        self.links.push(Link {
            a,
            b,
            path,
            drawn: 0,
            kind: LinkKind::Cross,
            load: 0,
            breach_ttl: 0,
        });
        self.push_log(format!(
            "bridge {}↔{} established",
            self.nodes[a].branch_id, self.nodes[b].branch_id
        ));
    }

    fn roll_role(&mut self) -> Role {
        let w = &self.cfg.role_weights;
        let total = w.relay
            + w.scanner
            + w.exfil
            + w.honeypot
            + w.defender
            + w.tower
            + w.beacon
            + w.proxy
            + w.decoy;
        let mut r = self.rng.gen::<f32>() * total.max(f32::EPSILON);
        if r < w.relay {
            return Role::Relay;
        }
        r -= w.relay;
        if r < w.scanner {
            return Role::Scanner;
        }
        r -= w.scanner;
        if r < w.exfil {
            return Role::Exfil;
        }
        r -= w.exfil;
        if r < w.honeypot {
            return Role::Honeypot;
        }
        r -= w.honeypot;
        if r < w.defender {
            return Role::Defender;
        }
        r -= w.defender;
        if r < w.tower {
            return Role::Tower;
        }
        r -= w.tower;
        if r < w.beacon {
            return Role::Beacon;
        }
        r -= w.beacon;
        if r < w.proxy {
            return Role::Proxy;
        }
        Role::Decoy
    }

    fn alloc_branch_id(&mut self) -> u16 {
        let id = self.next_branch_id;
        self.next_branch_id = self.next_branch_id.wrapping_add(1).max(1);
        id
    }

    /// Map each node to the index of its inbound parent link, if any.
    /// Cross-links are deliberately skipped — packets ride parent chains
    /// only, and cascade reachability has its own adjacency builder.
    fn build_inbound_links(&self) -> HashMap<NodeId, usize> {
        let mut inbound: HashMap<NodeId, usize> = HashMap::new();
        for (li, link) in self.links.iter().enumerate() {
            if link.kind == LinkKind::Parent {
                inbound.insert(link.b, li);
            }
        }
        inbound
    }
}

/// Truncate a mesh position to a pair of IP-like octets for log lines.
fn octet_pair(pos: (i16, i16)) -> (u8, u8) {
    ((pos.0 as u32 & 0xff) as u8, (pos.1 as u32 & 0xff) as u8)
}

impl World {

    fn try_spawn(&mut self) {
        if self.nodes.len() >= self.cfg.max_nodes {
            return;
        }
        let mut spawn_mult = if self.is_night() {
            self.cfg.night_spawn_mult
        } else {
            1.0
        };
        if self.is_storming() {
            spawn_mult *= self.cfg.storm_spawn_mult;
        }
        let effective_spawn = (self.cfg.p_spawn * spawn_mult).clamp(0.0, 1.0);
        if !self.rng.gen_bool(effective_spawn as f64) {
            return;
        }

        // Weighted pick over Alive nodes, favoring recent births. C2 gets a
        // constant weight floor so it remains a viable parent throughout the
        // run — otherwise its age-decayed weight collapses below the frontier
        // and the mesh stops minting new branches after the first ~30 ticks.
        let now = self.tick;
        let c2_bias = self.cfg.c2_spawn_bias;
        let c2_set: HashSet<NodeId> = self.c2_nodes.iter().copied().collect();
        // Collect beacon positions so we can apply a spawn weight
        // bonus to candidates within beacon_radius.
        let beacon_positions: Vec<(i16, i16)> = self
            .nodes
            .iter()
            .filter_map(|n| {
                if matches!(n.state, State::Alive) && n.role == Role::Beacon {
                    Some(n.pos)
                } else {
                    None
                }
            })
            .collect();
        let beacon_radius = self.cfg.beacon_radius;
        let beacon_mult = self.cfg.beacon_weight_mult;
        let mut candidates: Vec<(NodeId, f32)> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| match n.state {
                State::Alive => {
                    let mut weight = if c2_set.contains(&i) {
                        c2_bias
                    } else {
                        let age = (now - n.born) as f32;
                        1.0 / (1.0 + age * 0.1)
                    };
                    // Beacon aura: stack a multiplicative bonus for
                    // each beacon this candidate sits inside the radius
                    // of, creating clusters near rally points.
                    for bpos in &beacon_positions {
                        let d = (bpos.0 - n.pos.0).abs().max((bpos.1 - n.pos.1).abs());
                        if d <= beacon_radius {
                            weight *= beacon_mult;
                        }
                    }
                    Some((i, weight))
                }
                _ => None,
            })
            .collect();
        if candidates.is_empty() {
            return;
        }
        let total: f32 = candidates.iter().map(|(_, w)| *w).sum();
        let mut pick = self.rng.gen::<f32>() * total;
        let mut parent_id = candidates[0].0;
        for (id, w) in candidates.drain(..) {
            if pick <= w {
                parent_id = id;
                break;
            }
            pick -= w;
        }

        let parent_pos = self.nodes[parent_id].pos;
        let depth = self.depth_of(parent_id);
        let dist = (self.cfg.base_dist + (depth as i16) / 4).clamp(3, 8);

        // Scanner directional bias: if the parent is a Scanner that recently
        // pinged, grow toward the ping direction instead of rolling a new dir.
        let dir = {
            let parent = &self.nodes[parent_id];
            let ping_window = self.cfg.scanner_ping_period as u64 / 2;
            let recent_ping = self.tick.saturating_sub(parent.last_ping_tick) < ping_window;
            match parent.last_ping_dir {
                Some((dx, dy)) if parent.role == Role::Scanner && recent_ping => {
                    (dx as i16, dy as i16)
                }
                _ => DIRS[self.rng.gen_range(0..DIRS.len())],
            }
        };
        let cand = (parent_pos.0 + dir.0 * dist, parent_pos.1 + dir.1 * dist);

        // Border clamp.
        if cand.0 <= 0 || cand.1 <= 0 || cand.0 >= self.bounds.0 - 1 || cand.1 >= self.bounds.1 - 1
        {
            return;
        }
        // Collision checks.
        if self.occupied.contains(&cand) {
            return;
        }
        let min_gap = 2;
        for n in &self.nodes {
            let dx = (n.pos.0 - cand.0).abs();
            let dy = (n.pos.1 - cand.1).abs();
            if dx.max(dy) < min_gap {
                return;
            }
        }

        let path = match routing::route_link(
            parent_pos,
            cand,
            &self.occupied,
            self.bounds,
            &mut self.rng,
        ) {
            Some(p) => p,
            None => return,
        };

        // Branch id: first-hop children of any C2 each spawn a fresh branch,
        // and any other spawn occasionally forks off into its own sub-botnet
        // via the configurable fork_rate roll. Otherwise the new node
        // inherits its parent's branch.
        let parent_is_c2 = self.is_c2(parent_id);
        let forks = parent_is_c2
            || (self.cfg.fork_rate > 0.0 && self.rng.gen_bool(self.cfg.fork_rate as f64));
        let branch_id = if forks {
            self.alloc_branch_id()
        } else {
            self.nodes[parent_id].branch_id
        };
        let mut role = self.roll_role();
        let faction = self.nodes[parent_id].faction;

        // Towers only spawn near their faction's C2. If we rolled Tower
        // but the candidate cell is too far from any C2, fall back to
        // Relay so the fortified core stays a fortified core.
        if role == Role::Tower {
            let radius = self.cfg.tower_spawn_radius;
            let near_c2 = self.c2_nodes.iter().any(|&id| {
                let p = self.nodes[id].pos;
                (p.0 - cand.0).abs().max((p.1 - cand.1).abs()) <= radius
            });
            if !near_c2 {
                role = Role::Relay;
            }
        }

        let new_id = self.nodes.len();
        let mut node = Node::fresh(cand, Some(parent_id), self.tick, role, branch_id);
        node.faction = faction;
        if role == Role::Tower {
            node.pwn_resist = self.cfg.tower_pwn_resist;
        }
        self.nodes.push(node);
        if let Some(s) = self.faction_stats.get_mut(faction as usize) {
            s.spawned += 1;
        }
        self.occupied.insert(cand);
        self.links.push(Link {
            a: parent_id,
            b: new_id,
            path,
            drawn: 0,
            kind: LinkKind::Parent,
            load: 0,
            breach_ttl: 0,
        });

        let h = (cand.0 as u32).wrapping_mul(2654435761) ^ (cand.1 as u32).wrapping_mul(40503);
        let a = (h >> 16) & 0xff;
        let b = (h >> 8) & 0xff;
        let c = h & 0xff;
        self.push_log(format!("handshake 10.{}.{}.{} OK", a, b, c));
    }

    fn depth_of(&self, mut id: NodeId) -> u32 {
        let mut d = 0;
        let mut guard = 0;
        while let Some(p) = self.nodes[id].parent {
            id = p;
            d += 1;
            guard += 1;
            if guard > 1024 {
                break;
            }
        }
        d
    }

    fn advance_links(&mut self) {
        let step_amount: u16 = if self.tick.is_multiple_of(2) { 1 } else { 2 };
        for link in self.links.iter_mut() {
            let total = link.path.len() as u16;
            if link.drawn >= total {
                continue;
            }
            // Skip animation if endpoint is dead.
            let b_state = self.nodes[link.b].state;
            if matches!(b_state, State::Dead) {
                continue;
            }
            let next = (link.drawn + step_amount).min(total);
            for i in link.drawn as usize..next as usize {
                let c = link.path[i];
                if i != link.path.len() - 1 {
                    self.occupied.insert(c);
                }
            }
            link.drawn = next;
        }
    }

    fn heartbeat(&mut self) {
        if self.tick > 0 && self.tick.is_multiple_of(self.cfg.heartbeat_period) {
            let threshold = self.cfg.hardened_after_heartbeats;
            let mut newly_hardened: Vec<(i16, i16)> = Vec::new();
            // Emit a patch wave from each C2 alongside the beacon pulse.
            let c2_positions: Vec<(i16, i16)> =
                self.c2_nodes.iter().map(|&id| self.nodes[id].pos).collect();
            for pos in c2_positions {
                self.patch_waves.push(PatchWave {
                    origin: pos,
                    radius: 0,
                });
            }
            for n in self.nodes.iter_mut() {
                if matches!(n.state, State::Alive) {
                    n.pulse = 2;
                    n.heartbeats = n.heartbeats.saturating_add(1);
                    if !n.hardened && n.heartbeats >= threshold {
                        n.hardened = true;
                        newly_hardened.push(n.pos);
                    }
                }
            }
            self.push_log(format!("beacon sweep @ t={}", self.tick));
            for pos in newly_hardened {
                self.log_node(pos, "hardened");
            }
        } else {
            for n in self.nodes.iter_mut() {
                if n.pulse > 0 {
                    n.pulse -= 1;
                }
            }
        }
    }

    fn advance_role_cooldowns(&mut self) {
        for n in self.nodes.iter_mut() {
            if n.role_cooldown > 0 {
                n.role_cooldown -= 1;
            }
            if n.honey_reveal > 0 {
                n.honey_reveal -= 1;
            }
            if n.shield_flash > 0 {
                n.shield_flash -= 1;
            }
            if n.mutated_flash > 0 {
                n.mutated_flash -= 1;
            }
            if n.scan_pulse > 0 {
                n.scan_pulse -= 1;
            }
        }
    }

    fn fire_scanner_pings(&mut self) {
        let period = self.cfg.scanner_ping_period;
        let now = self.tick;
        let scanner_ids: Vec<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                if matches!(n.state, State::Alive)
                    && n.role == Role::Scanner
                    && n.role_cooldown == 0
                    && !n.role_suppressed()
                {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();
        let mut fired_positions: Vec<(i16, i16)> = Vec::new();
        for id in scanner_ids {
            // Pick a direction so the spawn bias in try_spawn still favors
            // growth along the scanner's last sweep. Pulled out of the
            // mut borrow below to avoid aliasing.
            let dir_idx = self.rng.gen_range(0..DIRS.len());
            let (dx, dy) = DIRS[dir_idx];
            let n = &mut self.nodes[id];
            n.role_cooldown = period;
            n.last_ping_tick = now;
            n.last_ping_dir = Some((dx as i8, dy as i8));
            // Light up the scanner itself and its adjacent links for a few
            // ticks. The render pass reads scan_pulse to brighten the node
            // and every link touching it.
            n.scan_pulse = SCANNER_PULSE_TICKS;
            fired_positions.push(n.pos);
        }
        // Proxies within proxy_radius of any fired scanner echo the
        // pulse on the same tick, so a single scanner firing lights
        // up a chain of connected proxies.
        if !fired_positions.is_empty() {
            let radius = self.cfg.proxy_radius;
            for n in self.nodes.iter_mut() {
                if !matches!(n.state, State::Alive) || n.role != Role::Proxy {
                    continue;
                }
                for fpos in &fired_positions {
                    let d = (fpos.0 - n.pos.0).abs().max((fpos.1 - n.pos.1).abs());
                    if d <= radius {
                        n.scan_pulse = SCANNER_PULSE_TICKS;
                        break;
                    }
                }
            }
        }
    }

    fn fire_exfil_packets(&mut self) {
        let period = self.cfg.exfil_packet_period;
        let inbound = self.build_inbound_links();
        let exfil_ids: Vec<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                if matches!(n.state, State::Alive)
                    && n.role == Role::Exfil
                    && n.role_cooldown == 0
                    && !n.honey_tripped
                    && !n.role_suppressed()
                {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();
        for id in exfil_ids {
            self.nodes[id].role_cooldown = period;
            if let Some(&link_id) = inbound.get(&id) {
                let link = &self.links[link_id];
                if link.path.is_empty() {
                    continue;
                }
                self.packets.push(Packet {
                    link_id,
                    pos: (link.path.len() - 1) as u16,
                });
            }
        }
    }

    fn fire_defender_pulses(&mut self) {
        let period = self.cfg.defender_pulse_period;
        let radius = self.cfg.defender_radius;
        // Active defenders ready to pulse this tick.
        let defenders: Vec<(NodeId, (i16, i16))> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                if matches!(n.state, State::Alive)
                    && n.role == Role::Defender
                    && n.role_cooldown == 0
                {
                    Some((i, n.pos))
                } else {
                    None
                }
            })
            .collect();
        if defenders.is_empty() {
            return;
        }
        let mut cured_positions: Vec<((i16, i16), u8)> = Vec::new();
        for (id, dpos) in defenders {
            self.nodes[id].role_cooldown = period;
            self.nodes[id].pulse = 2;
            // Scan for infected neighbors within radius and decrement
            // their cure_resist; clear infections that drop to zero.
            for n in self.nodes.iter_mut() {
                let Some(inf) = n.infection.as_mut() else {
                    continue;
                };
                let dx = (n.pos.0 - dpos.0).abs();
                let dy = (n.pos.1 - dpos.1).abs();
                if dx.max(dy) > radius {
                    continue;
                }
                if inf.cure_resist <= 1 {
                    cured_positions.push((n.pos, n.faction));
                    n.infection = None;
                } else {
                    inf.cure_resist -= 1;
                }
            }
        }
        for (pos, faction) in cured_positions {
            self.log_node(pos, "patched");
            if let Some(s) = self.faction_stats.get_mut(faction as usize) {
                s.infections_cured += 1;
            }
        }
    }

    fn advance_sparks(&mut self) {
        for s in self.sparks.iter_mut() {
            s.pos.0 += s.vel.0;
            s.pos.1 += s.vel.1;
            // Friction so sparks slow down and cluster near their
            // final positions instead of flying off forever.
            s.vel.0 *= 0.86;
            s.vel.1 *= 0.86;
            s.age = s.age.saturating_add(1);
        }
        self.sparks.retain(|s| s.age < s.life);
    }

    fn advance_shockwaves(&mut self) {
        for sw in self.shockwaves.iter_mut() {
            sw.age = sw.age.saturating_add(1);
        }
        self.shockwaves.retain(|sw| sw.age <= sw.max_age);
    }

    /// Emit a burst of sparks and a shockwave at the cascade root.
    /// Called from schedule_subtree_death when a cascade actually
    /// finalized a nonzero number of hosts.
    fn emit_cascade_effects(&mut self, root_pos: (i16, i16), touched: u32) {
        // Shockwave: radius scaled to cascade size, capped.
        let max_age = (touched / 3).clamp(3, 10) as u8;
        self.shockwaves.push(CascadeShockwave {
            origin: root_pos,
            age: 0,
            max_age,
        });
        // Sparks: 8 plus 1 per 5 hosts, capped at 24.
        let count = (8 + (touched / 5)).min(24);
        let origin_x = root_pos.0 as f32 + 0.5;
        let origin_y = root_pos.1 as f32 + 0.5;
        for _ in 0..count {
            let angle = self.rng.gen::<f32>() * std::f32::consts::TAU;
            let speed = 0.6 + self.rng.gen::<f32>() * 0.8;
            let vx = angle.cos() * speed;
            let vy = angle.sin() * speed * 0.6; // flatter vertically since cells are ~2x tall
            let life = 7 + self.rng.gen_range(0..4) as u8;
            self.sparks.push(CascadeSpark {
                pos: (origin_x, origin_y),
                vel: (vx, vy),
                age: 0,
                life,
            });
        }
    }

    /// Decay one step of traffic load and breach TTL from every link.
    /// Called at the top of the motion phase so the add/decay pair stays
    /// symmetric.
    fn decay_link_load(&mut self) {
        for link in self.links.iter_mut() {
            link.load = link.load.saturating_sub(1);
            link.breach_ttl = link.breach_ttl.saturating_sub(1);
        }
    }

    /// Roll for a network storm. Storms spike both spawn and loss rates
    /// for a configurable duration and log the start / end transitions.
    fn maybe_storm(&mut self) {
        // End an active storm when its window elapses.
        if self.storm_until > 0 && self.tick >= self.storm_until {
            self.storm_until = 0;
            self.push_log("storm passes — mesh settling".to_string());
            return;
        }
        // Roll for a new storm only when one isn't already active, and
        // only once per storm_period, skipping tick 0 so fresh worlds
        // don't storm on frame one.
        let period = self.cfg.storm_period;
        if period == 0 || self.storm_until > 0 || self.tick == 0 {
            return;
        }
        if !self.tick.is_multiple_of(period) {
            return;
        }
        if !self.rng.gen_bool(self.cfg.storm_chance as f64) {
            return;
        }
        self.storm_until = self.tick + self.cfg.storm_duration;
        self.push_log("⚡ STORM — mesh destabilizing".to_string());
    }

    /// Walk up the parent chain from `victim` toward C2, marking each
    /// link we traverse as part of an exploit chain breach. The result
    /// reads as a visible trail of red-tinted wires leading back to C2
    /// from the fresh kill — the story of how the attack got here.
    fn breach_chain_up(&mut self, victim: NodeId) {
        let mut cur = victim;
        let mut hops = 0;
        while hops < BREACH_MAX_HOPS {
            let Some(parent_id) = self.nodes[cur].parent else {
                break;
            };
            // Find the parent-link connecting cur to parent_id.
            let mut found = None;
            for (i, l) in self.links.iter().enumerate() {
                if l.kind == LinkKind::Parent && l.a == parent_id && l.b == cur {
                    found = Some(i);
                    break;
                }
            }
            if let Some(link_id) = found {
                self.links[link_id].breach_ttl = BREACH_TTL;
            }
            if self.is_c2(parent_id) {
                break;
            }
            cur = parent_id;
            hops += 1;
        }
    }

    fn advance_packets(&mut self) {
        if self.packets.is_empty() {
            return;
        }
        let inbound = self.build_inbound_links();

        let mut keep: Vec<Packet> = Vec::with_capacity(self.packets.len());
        let mut dropped_positions: Vec<(i16, i16)> = Vec::new();
        for mut pkt in std::mem::take(&mut self.packets) {
            let (link_a, link_b) = {
                let link = &self.links[pkt.link_id];
                (link.a, link.b)
            };
            let a_state = self.nodes[link_a].state;
            let b_state = self.nodes[link_b].state;
            let a_dying = self.nodes[link_a].dying_in > 0;
            let b_dying = self.nodes[link_b].dying_in > 0;
            if matches!(a_state, State::Dead)
                || matches!(b_state, State::Dead)
                || a_dying
                || b_dying
            {
                continue; // drop packet; route is compromised
            }
            // Each in-flight packet heats up its current link.
            self.links[pkt.link_id].load =
                self.links[pkt.link_id].load.saturating_add(PACKET_LOAD_INCREMENT);
            if pkt.pos == 0 {
                // Reached the parent end of this link. Hop to the parent's
                // own inbound link, or drop if parent is C2.
                let parent_id = link_a;
                if self.is_c2(parent_id) {
                    continue; // delivered
                }
                if let Some(&next_link) = inbound.get(&parent_id) {
                    let next = &self.links[next_link];
                    if next.path.is_empty() {
                        continue;
                    }
                    if next.load >= HOT_LINK {
                        // Congested downstream leg — drop the packet.
                        dropped_positions.push(self.nodes[parent_id].pos);
                        continue;
                    }
                    pkt.link_id = next_link;
                    pkt.pos = (next.path.len() - 1) as u16;
                    keep.push(pkt);
                }
                continue;
            }
            pkt.pos -= 1;
            keep.push(pkt);
        }
        self.packets = keep;
        for pos in dropped_positions {
            self.log_node(pos, "pkt drop (hot)");
        }
    }

    fn advance_worms(&mut self) {
        if self.worms.is_empty() {
            return;
        }
        // Worms crawl at half the sim rate so each cell is visible long enough
        // to register. On off-ticks we still run the compromised-link drop
        // check so dead links clean up promptly.
        let move_tick = self.tick.is_multiple_of(WORM_STEP_INTERVAL);
        let cure_resist = self.cfg.virus_cure_resist;
        let c2_set: HashSet<NodeId> = self.c2_nodes.iter().copied().collect();
        let mut keep: Vec<Worm> = Vec::with_capacity(self.worms.len());
        let mut arrivals: Vec<(NodeId, u8, (i16, i16))> = Vec::new();
        for mut worm in std::mem::take(&mut self.worms) {
            let (link_a, link_b, link_len) = {
                let link = &self.links[worm.link_id];
                (link.a, link.b, link.path.len())
            };
            // Drop the worm if its carrier link is compromised.
            let a_node = &self.nodes[link_a];
            let b_node = &self.nodes[link_b];
            if matches!(a_node.state, State::Dead)
                || matches!(b_node.state, State::Dead)
                || a_node.dying_in > 0
                || b_node.dying_in > 0
            {
                continue;
            }
            // Each in-flight worm contributes to its carrier link's load.
            self.links[worm.link_id].load =
                self.links[worm.link_id].load.saturating_add(WORM_LOAD_INCREMENT);
            if !move_tick {
                keep.push(worm);
                continue;
            }
            if worm.outbound_from_a {
                let next = worm.pos as usize + 1;
                if next >= link_len {
                    let target = link_b;
                    if !c2_set.contains(&target)
                        && matches!(self.nodes[target].state, State::Alive)
                        && self.nodes[target].infection.is_none()
                    {
                        arrivals.push((target, worm.strain, self.nodes[target].pos));
                    }
                    continue;
                }
                worm.pos = next as u16;
            } else {
                if worm.pos == 0 {
                    let target = link_a;
                    if !c2_set.contains(&target)
                        && matches!(self.nodes[target].state, State::Alive)
                        && self.nodes[target].infection.is_none()
                    {
                        arrivals.push((target, worm.strain, self.nodes[target].pos));
                    }
                    continue;
                }
                worm.pos -= 1;
            }
            keep.push(worm);
        }
        self.worms = keep;
        for (target, strain, pos) in arrivals {
            self.nodes[target].infection = Some(Infection::seeded(strain, cure_resist));
            let (a, b) = octet_pair(pos);
            let name = self.strain_name(strain);
            self.push_log(format!("worm delivered {} @ 10.0.{}.{}", name, a, b));
        }
    }

    fn maybe_spawn_worms(&mut self) {
        let rate = self.cfg.worm_spawn_rate;
        if rate <= 0.0 {
            return;
        }
        // Find active-infected carriers up front.
        let carriers: Vec<(NodeId, u8)> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                if self.is_c2(i) || !matches!(n.state, State::Alive) {
                    return None;
                }
                match n.infection {
                    Some(inf) if matches!(inf.stage, InfectionStage::Active) => {
                        Some((i, inf.strain))
                    }
                    _ => None,
                }
            })
            .collect();
        for (id, strain) in carriers {
            if !self.rng.gen_bool(rate as f64) {
                continue;
            }
            // Outgoing links from this node (either direction for Cross).
            let outgoing: Vec<(usize, bool)> = self
                .links
                .iter()
                .enumerate()
                .filter_map(|(li, l)| {
                    if (l.drawn as usize) < l.path.len() {
                        return None;
                    }
                    if l.a == id {
                        Some((li, true))
                    } else if l.b == id {
                        Some((li, false))
                    } else {
                        None
                    }
                })
                .collect();
            if outgoing.is_empty() {
                continue;
            }
            let (link_id, from_a) = outgoing[self.rng.gen_range(0..outgoing.len())];
            let link = &self.links[link_id];
            let target = if from_a { link.b } else { link.a };
            if self.is_c2(target) {
                continue;
            }
            if !matches!(self.nodes[target].state, State::Alive) {
                continue;
            }
            if self.nodes[target].infection.is_some() {
                continue;
            }
            // Start one cell in from the carrier node so the worm is visible
            // on its spawn tick (cell 0 / len-1 are the endpoint positions
            // which the renderer skips to avoid colliding with node glyphs).
            let len = link.path.len();
            if len < 2 {
                continue;
            }
            let pos = if from_a { 1 } else { (len - 2) as u16 };
            let carrier_pos = self.nodes[id].pos;
            self.worms.push(Worm {
                link_id,
                pos,
                outbound_from_a: from_a,
                strain,
            });
            let (a, b) = octet_pair(carrier_pos);
            self.push_log(format!("worm launched from 10.0.{}.{}", a, b));
        }
    }

    fn advance_patch_waves(&mut self) {
        if self.patch_waves.is_empty() {
            return;
        }
        let max_r = self.cfg.patch_wave_radius;
        for wave in self.patch_waves.iter_mut() {
            wave.radius += 1;
        }
        // Snapshot wave geometry so we can mutably borrow self.nodes.
        let geo: Vec<(i16, i16, i16)> = self
            .patch_waves
            .iter()
            .map(|w| (w.origin.0, w.origin.1, w.radius))
            .collect();
        let mut cured: Vec<((i16, i16), u8)> = Vec::new();
        for n in self.nodes.iter_mut() {
            if n.infection.is_none() {
                continue;
            }
            for &(ox, oy, r) in &geo {
                let dist = (n.pos.0 - ox).abs().max((n.pos.1 - oy).abs());
                // The wave front is a single ring at Chebyshev distance == r.
                // Each node sees the wave exactly once per pass, so a single
                // wave decrements cure_resist by exactly 1.
                if dist == r {
                    let Some(inf) = n.infection.as_mut() else {
                        break;
                    };
                    if inf.cure_resist <= 1 {
                        cured.push((n.pos, n.faction));
                        n.infection = None;
                        break;
                    } else {
                        inf.cure_resist -= 1;
                    }
                }
            }
        }
        self.patch_waves.retain(|w| w.radius <= max_r);
        for (pos, faction) in cured {
            self.log_node(pos, "cured");
            if let Some(s) = self.faction_stats.get_mut(faction as usize) {
                s.infections_cured += 1;
            }
        }
    }

    fn advance_infections(&mut self) {
        // Cache config values so the mut-borrow loop below doesn't need &self.
        let incubation = self.cfg.virus_incubation_ticks;
        let active_len = self.cfg.virus_active_ticks;
        let terminal_len = self.cfg.virus_terminal_ticks;

        // Pass 1: stage advancement + terminal expiry collection.
        let mut to_pwn: Vec<NodeId> = Vec::new();
        let mut newly_active: Vec<(i16, i16)> = Vec::new();
        for (id, n) in self.nodes.iter_mut().enumerate() {
            if !matches!(n.state, State::Alive) {
                continue;
            }
            let Some(inf) = n.infection.as_mut() else {
                continue;
            };
            inf.age = inf.age.saturating_add(1);
            match inf.stage {
                InfectionStage::Incubating => {
                    if inf.age >= incubation {
                        inf.stage = InfectionStage::Active;
                        newly_active.push(n.pos);
                    }
                }
                InfectionStage::Active => {
                    if inf.age >= incubation + active_len {
                        inf.stage = InfectionStage::Terminal;
                        inf.terminal_ticks = terminal_len;
                    }
                }
                InfectionStage::Terminal => {
                    if inf.terminal_ticks <= 1 {
                        to_pwn.push(id);
                    } else {
                        inf.terminal_ticks -= 1;
                    }
                }
            }
        }

        // Pass 2: spread. Walk the cascade adjacency; each uninfected alive
        // node with infected neighbors rolls once per tick. We collect first
        // and apply after so freshly infected nodes don't re-infect siblings
        // in the same tick.
        let spread_rate = self.cfg.virus_spread_rate;
        let cure_resist = self.cfg.virus_cure_resist;
        let c2_set: HashSet<NodeId> = self.c2_nodes.iter().copied().collect();
        let adj = self.live_adjacency();
        let mut newly_infected: Vec<(NodeId, u8)> = Vec::new();
        if spread_rate > 0.0 {
            for (id, n) in self.nodes.iter().enumerate() {
                if c2_set.contains(&id) {
                    continue;
                }
                if !matches!(n.state, State::Alive) || n.infection.is_some() {
                    continue;
                }
                // Honeypots stay clean so their disguise survives; defenders
                // are immune by design (they're the antibodies).
                if n.role == Role::Honeypot || n.role == Role::Defender {
                    continue;
                }
                let Some(neighbors) = adj.get(&id) else {
                    continue;
                };
                let mut tally: [u32; STRAIN_COUNT] = [0; STRAIN_COUNT];
                let mut infected_count: u32 = 0;
                for &m in neighbors {
                    if let Some(inf) = self.nodes[m].infection {
                        if !matches!(inf.stage, InfectionStage::Incubating) {
                            tally[(inf.strain as usize) % STRAIN_COUNT] += 1;
                            infected_count += 1;
                        }
                    }
                }
                if infected_count == 0 {
                    continue;
                }
                let p = 1.0 - (1.0 - spread_rate).powi(infected_count as i32);
                if self.rng.gen::<f32>() < p {
                    let strain = tally
                        .iter()
                        .enumerate()
                        .max_by_key(|(_, c)| **c)
                        .map(|(i, _)| i as u8)
                        .unwrap_or(0);
                    newly_infected.push((id, strain));
                }
            }
        }
        for (id, strain) in newly_infected {
            self.nodes[id].infection = Some(Infection::seeded(strain, cure_resist));
        }

        // Terminal nodes crash the host — route into the loss/cascade pipeline.
        let pwned_flash = self.cfg.pwned_flash_ticks;
        for id in to_pwn {
            let pos = self.nodes[id].pos;
            let node = &mut self.nodes[id];
            node.infection = None;
            node.state = State::Pwned {
                ticks_left: pwned_flash,
            };
            self.log_node(pos, "necrotic");
        }

        for pos in newly_active {
            self.log_node(pos, "symptomatic");
        }

        // Mythic: PANDEMIC fires once per run if every one of the
        // STRAIN_COUNT strains is simultaneously alive in the mesh.
        if !self.mythic_pandemic_seen {
            let mut seen = [false; STRAIN_COUNT];
            for n in &self.nodes {
                if let Some(inf) = n.infection {
                    seen[(inf.strain as usize) % STRAIN_COUNT] = true;
                }
            }
            if seen.iter().all(|s| *s) {
                self.mythic_pandemic_seen = true;
                self.push_log("✦ MYTHIC ✦ PANDEMIC — all strains active".to_string());
            }
        }
    }

    fn maybe_seed_infection(&mut self) {
        if self.cfg.virus_seed_rate <= 0.0 {
            return;
        }
        if self.nodes.iter().any(|n| n.infection.is_some()) {
            return;
        }
        if !self.rng.gen_bool(self.cfg.virus_seed_rate as f64) {
            return;
        }
        let candidates: Vec<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                if !self.is_c2(i)
                    && matches!(n.state, State::Alive)
                    && n.role != Role::Honeypot
                    && n.role != Role::Defender
                {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();
        if candidates.is_empty() {
            return;
        }
        let id = candidates[self.rng.gen_range(0..candidates.len())];
        let strain = self.rng.gen_range(0..STRAIN_COUNT as u8);
        let cure_resist = self.cfg.virus_cure_resist;
        self.nodes[id].infection = Some(Infection::seeded(strain, cure_resist));
        let pos = self.nodes[id].pos;
        let (a, b) = octet_pair(pos);
        let name = self.strain_name(strain);
        self.push_log(format!("{} detected at 10.0.{}.{}", name, a, b));
    }

    fn maybe_mutate(&mut self) {
        let rate = self.cfg.mutate_rate;
        if rate <= 0.0 {
            return;
        }
        let min_age = self.cfg.mutate_min_age;
        let now = self.tick;
        // Collect eligible candidates first to avoid aliasing rng borrow.
        let candidates: Vec<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                if self.is_c2(i) {
                    return None;
                }
                if !matches!(n.state, State::Alive) {
                    return None;
                }
                if n.infection.is_some() {
                    return None;
                }
                if matches!(
                    n.role,
                    Role::Honeypot
                        | Role::Defender
                        | Role::Tower
                        | Role::Beacon
                        | Role::Proxy
                        | Role::Decoy
                ) {
                    return None; // specialized roles stay in their lane
                }
                if now.saturating_sub(n.born) < min_age {
                    return None;
                }
                Some(i)
            })
            .collect();
        for id in candidates {
            if !self.rng.gen_bool(rate as f64) {
                continue;
            }
            let current = self.nodes[id].role;
            let choices: [Role; 3] = match current {
                Role::Relay => [Role::Scanner, Role::Exfil, Role::Relay],
                Role::Scanner => [Role::Relay, Role::Exfil, Role::Scanner],
                Role::Exfil => [Role::Relay, Role::Scanner, Role::Exfil],
                Role::Honeypot
                | Role::Defender
                | Role::Tower
                | Role::Beacon
                | Role::Proxy
                | Role::Decoy => continue,
            };
            // Pick uniformly from the first two (the third is the sentinel).
            let new_role = choices[self.rng.gen_range(0..2)];
            let pos = self.nodes[id].pos;
            self.nodes[id].role = new_role;
            self.nodes[id].mutated_flash = 6;
            let name = match new_role {
                Role::Relay => "relay",
                Role::Scanner => "scanner",
                Role::Exfil => "exfil",
                Role::Honeypot => "honeypot",
                Role::Defender => "defender",
                Role::Tower => "tower",
                Role::Beacon => "beacon",
                Role::Proxy => "proxy",
                Role::Decoy => "decoy",
            };
            self.log_node(pos, &format!("mutated → {}", name));
        }
    }

    fn maybe_zero_day(&mut self) {
        let period = self.cfg.zero_day_period;
        if period == 0 || self.cfg.zero_day_chance <= 0.0 {
            return;
        }
        if self.tick == 0 || !self.tick.is_multiple_of(period) {
            return;
        }
        let alive_count = self
            .nodes
            .iter()
            .filter(|n| matches!(n.state, State::Alive))
            .count();
        if alive_count < ZERO_DAY_MIN_ALIVE {
            return;
        }
        if !self.rng.gen_bool(self.cfg.zero_day_chance as f64) {
            return;
        }
        // Mythic: zero-day coinciding with an active storm.
        if self.is_storming() {
            self.push_log("✦ MYTHIC ✦ CONFLUENCE — zero-day amid storm".to_string());
        }
        let roll = self.rng.gen::<f32>();
        if roll < ZERO_DAY_OUTBREAK_WEIGHT {
            self.zero_day_outbreak();
        } else if roll < ZERO_DAY_PATCH_WEIGHT {
            self.zero_day_emergency_patch();
        } else {
            self.zero_day_immune_breakthrough();
        }
    }

    fn zero_day_outbreak(&mut self) {
        let strain = self.rng.gen_range(0..STRAIN_COUNT as u8);
        let count = self.rng.gen_range(ZERO_DAY_OUTBREAK_MIN..=ZERO_DAY_OUTBREAK_MAX);
        let cure_resist = self.cfg.virus_cure_resist.saturating_mul(2);
        let mut candidates: Vec<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                if !self.is_c2(i)
                    && matches!(n.state, State::Alive)
                    && n.infection.is_none()
                    && n.role != Role::Honeypot
                    && n.role != Role::Defender
                {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();
        if candidates.is_empty() {
            return;
        }
        // Shuffle and take so we hit `count` distinct nodes (or all of them
        // if fewer candidates exist) without picking the same id twice.
        candidates.shuffle(&mut self.rng);
        let take = (count as usize).min(candidates.len());
        for &id in candidates.iter().take(take) {
            self.nodes[id].infection = Some(Infection::seeded(strain, cure_resist));
        }
        let name = self.strain_name(strain);
        self.push_log(format!("ZERO-DAY: {} outbreak ({})", name, take));
    }

    fn zero_day_emergency_patch(&mut self) {
        let mut cleared = 0u32;
        for n in self.nodes.iter_mut() {
            if let Some(inf) = n.infection {
                if matches!(inf.stage, InfectionStage::Incubating) {
                    n.infection = None;
                    cleared += 1;
                }
            }
        }
        self.push_log(format!(
            "ZERO-DAY: emergency patch ({} cleared)",
            cleared
        ));
    }

    fn zero_day_immune_breakthrough(&mut self) {
        // One-shot boost: raise cure_resist on any active infection so the
        // next patch wave won't clear them quite as fast. Mostly flavor.
        let mut boosted = 0u32;
        for n in self.nodes.iter_mut() {
            if let Some(inf) = n.infection.as_mut() {
                inf.cure_resist = inf.cure_resist.saturating_add(2);
                boosted += 1;
            }
        }
        self.push_log(format!(
            "ZERO-DAY: immune boost ({})",
            boosted
        ));
    }

    /// Infect a random Alive non-C2 non-Honeypot node with a fresh strain.
    /// Used by the `i` keybinding and by tests. Refuses to fire when the
    /// virus layer is disabled so --disable-virus really means "off".
    pub fn inject_infection(&mut self) -> Option<NodeId> {
        if self.cfg.virus_spread_rate <= 0.0 && self.cfg.virus_seed_rate <= 0.0 {
            self.push_log("inject refused: virus layer disabled".to_string());
            return None;
        }
        let candidates: Vec<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                if !self.is_c2(i)
                    && matches!(n.state, State::Alive)
                    && n.infection.is_none()
                    && n.role != Role::Honeypot
                    && n.role != Role::Defender
                {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();
        if candidates.is_empty() {
            return None;
        }
        let id = candidates[self.rng.gen_range(0..candidates.len())];
        let strain = self.rng.gen_range(0..STRAIN_COUNT as u8);
        let cure_resist = self.cfg.virus_cure_resist;
        self.nodes[id].infection = Some(Infection::seeded(strain, cure_resist));
        let pos = self.nodes[id].pos;
        let (a, b) = octet_pair(pos);
        let name = self.strain_name(strain);
        self.push_log(format!("INJECTED {} @ 10.0.{}.{}", name, a, b));
        Some(id)
    }

    fn advance_pwned_and_loss(&mut self) {
        // Tick down existing Pwned nodes.
        let mut to_schedule: Vec<NodeId> = Vec::new();
        for (i, n) in self.nodes.iter_mut().enumerate() {
            if let State::Pwned { ticks_left } = &mut n.state {
                if *ticks_left <= 1 {
                    to_schedule.push(i);
                } else {
                    *ticks_left -= 1;
                }
            }
        }
        for id in to_schedule {
            // Honeypot triggers an oversized cascade that also eats its parent.
            if self.nodes[id].role == Role::Honeypot && self.nodes[id].honey_tripped {
                // Reveal backdoor cross-links before cascading so the
                // shortcuts are visible for a few ticks before the death
                // wave propagates outward from them.
                self.reveal_honeypot_backdoors(id);
                if let Some(parent) = self.nodes[id].parent {
                    if !self.is_c2(parent) {
                        self.schedule_subtree_death(parent, self.cfg.honeypot_cascade_mult);
                        continue;
                    }
                }
                self.schedule_subtree_death(id, self.cfg.honeypot_cascade_mult);
            } else {
                self.schedule_subtree_death(id, 1.0);
            }
        }

        // Pick a new victim?
        let mut loss_mult = if self.is_night() {
            self.cfg.night_loss_mult
        } else {
            1.0
        };
        if self.is_storming() {
            loss_mult *= self.cfg.storm_loss_mult;
        }
        let effective_loss = (self.cfg.p_loss * loss_mult).clamp(0.0, 1.0);
        if self.rng.gen_bool(effective_loss as f64) {
            let alive_ids: Vec<NodeId> = self
                .nodes
                .iter()
                .enumerate()
                .filter_map(|(i, n)| {
                    if !self.is_c2(i) && matches!(n.state, State::Alive) {
                        Some(i)
                    } else {
                        None
                    }
                })
                .collect();
            if !alive_ids.is_empty() {
                let victim = alive_ids[self.rng.gen_range(0..alive_ids.len())];
                let pos = self.nodes[victim].pos;
                let node = &mut self.nodes[victim];

                if node.pwn_resist > 0 {
                    // Tower fortification absorbs the hit before any
                    // heartbeat shield or pwn even gets considered.
                    node.pwn_resist -= 1;
                    node.shield_flash = 6;
                    self.log_node(pos, "reinforced");
                } else if node.hardened {
                    // Reinforcement: consume the shield instead of pwning.
                    node.hardened = false;
                    node.heartbeats = 0;
                    node.shield_flash = 6;
                    self.log_node(pos, "shielded");
                } else if node.role == Role::Honeypot {
                    node.honey_tripped = true;
                    node.honey_reveal = 2;
                    node.state = State::Pwned {
                        ticks_left: self.cfg.pwned_flash_ticks,
                    };
                    let faction = node.faction;
                    let (a, b) = octet_pair(pos);
                    self.push_log(format!("HONEYPOT 10.0.{}.{} TRIPPED", a, b));
                    if let Some(s) = self.faction_stats.get_mut(faction as usize) {
                        s.honeys_tripped += 1;
                    }
                } else {
                    node.state = State::Pwned {
                        ticks_left: self.cfg.pwned_flash_ticks,
                    };
                    let faction = node.faction;
                    self.log_node(pos, "LOST");
                    if let Some(s) = self.faction_stats.get_mut(faction as usize) {
                        s.lost += 1;
                    }
                    // Trace the exploit chain back toward C2 so the
                    // path the attacker 'came from' glows red for a
                    // few ticks before the cascade catches up.
                    self.breach_chain_up(victim);
                }
            }
        }
    }

    /// Build the live undirected adjacency used for cascade reachability.
    /// Parent edges always count; cross edges only count once fully drawn.
    /// Dead / dying nodes are excluded entirely.
    fn live_adjacency(&self) -> HashMap<NodeId, Vec<NodeId>> {
        let mut adj: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let traversable = |id: NodeId| -> bool {
            let n = &self.nodes[id];
            !matches!(n.state, State::Dead) && n.dying_in == 0
        };
        for (id, n) in self.nodes.iter().enumerate() {
            if !traversable(id) {
                continue;
            }
            if let Some(p) = n.parent {
                if traversable(p) {
                    adj.entry(id).or_default().push(p);
                    adj.entry(p).or_default().push(id);
                }
            }
        }
        for link in &self.links {
            if link.kind != LinkKind::Cross {
                continue;
            }
            if (link.drawn as usize) < link.path.len() {
                continue;
            }
            if !traversable(link.a) || !traversable(link.b) {
                continue;
            }
            adj.entry(link.a).or_default().push(link.b);
            adj.entry(link.b).or_default().push(link.a);
        }
        adj
    }

    fn bfs_reachable(
        &self,
        start: NodeId,
        adj: &HashMap<NodeId, Vec<NodeId>>,
        forbidden: Option<NodeId>,
    ) -> HashSet<NodeId> {
        let mut seen: HashSet<NodeId> = HashSet::new();
        if Some(start) == forbidden {
            return seen;
        }
        let mut queue: VecDeque<NodeId> = VecDeque::new();
        queue.push_back(start);
        seen.insert(start);
        while let Some(id) = queue.pop_front() {
            if let Some(ns) = adj.get(&id) {
                for &m in ns {
                    if Some(m) == forbidden {
                        continue;
                    }
                    if seen.insert(m) {
                        queue.push_back(m);
                    }
                }
            }
        }
        seen
    }

    /// Compute which nodes should die when `root` is lost, and their
    /// BFS distance from `root` for cascade ordering. Uses a reachability
    /// diff anchored on the root's own faction's C2 — nodes in other
    /// factions are unaffected by this cascade, and cross-faction
    /// cross-links are filtered out by maybe_reconnect's same-faction
    /// constraint so the adjacency naturally stays within faction.
    fn compute_cascade(&self, root: NodeId) -> Vec<(NodeId, u8)> {
        let adj = self.live_adjacency();
        let faction = self.nodes[root].faction as usize;
        let anchor = self
            .c2_nodes
            .get(faction)
            .copied()
            .unwrap_or(self.c2_nodes[0]);
        let reach_with = self.bfs_reachable(anchor, &adj, None);
        let reach_without = self.bfs_reachable(anchor, &adj, Some(root));
        let doomed: HashSet<NodeId> = reach_with
            .difference(&reach_without)
            .copied()
            .collect();
        if doomed.is_empty() {
            return Vec::new();
        }
        let mut dist: HashMap<NodeId, u8> = HashMap::new();
        let mut queue: VecDeque<(NodeId, u8)> = VecDeque::new();
        queue.push_back((root, 0));
        dist.insert(root, 0);
        while let Some((id, d)) = queue.pop_front() {
            if let Some(ns) = adj.get(&id) {
                for &m in ns {
                    if !doomed.contains(&m) || dist.contains_key(&m) {
                        continue;
                    }
                    let nd = d.saturating_add(1);
                    dist.insert(m, nd);
                    queue.push_back((m, nd));
                }
            }
        }
        dist.into_iter().collect()
    }

    /// Stagger death through every node that loses its route to C2 when
    /// `root` is severed. Visible as a red wave radiating outward from the
    /// pwned node; cross-linked cousins survive. `mult` stretches the per-hop
    /// delay for theatrical effect — pass 1.0 for a normal cascade, higher
    /// values for a slower honeypot-style reveal.
    /// When a honeypot trips, reveal up to `honeypot_backdoor_max` new
    /// cross-links to nearby same-faction neighbors in different branches.
    /// The new links animate in normally (drawn: 0) so the viewer sees
    /// them reach outward before the cascade wave catches up.
    fn reveal_honeypot_backdoors(&mut self, honey_id: NodeId) {
        let max = self.cfg.honeypot_backdoor_max;
        if max == 0 {
            return;
        }
        let radius = self.cfg.honeypot_backdoor_radius;
        let a_pos = self.nodes[honey_id].pos;
        let a_branch = self.nodes[honey_id].branch_id;
        let a_faction = self.nodes[honey_id].faction;

        // Collect nearby eligible targets.
        let mut candidates: Vec<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                if i == honey_id {
                    return None;
                }
                if !matches!(n.state, State::Alive) || n.dying_in > 0 {
                    return None;
                }
                if n.faction != a_faction || n.branch_id == a_branch {
                    return None;
                }
                if self.is_c2(i) {
                    return None;
                }
                let dp = n.pos;
                let dist = (dp.0 - a_pos.0).abs().max((dp.1 - a_pos.1).abs());
                if dist > radius {
                    return None;
                }
                // Skip if a cross-link between honey and this node exists.
                let already = self.links.iter().any(|l| {
                    l.kind == LinkKind::Cross
                        && ((l.a == honey_id && l.b == i) || (l.a == i && l.b == honey_id))
                });
                if already {
                    return None;
                }
                Some(i)
            })
            .collect();
        if candidates.is_empty() {
            return;
        }
        candidates.shuffle(&mut self.rng);
        let count = self.rng.gen_range(1..=(max as usize));
        let take = count.min(candidates.len());

        // Routing wants the occupied set minus the two endpoints so it
        // doesn't reject the start/end cells. Same pattern the spawn
        // routing uses.
        let mut occ = self.occupied.clone();
        occ.remove(&a_pos);

        let bounds = self.bounds;
        let mut revealed = 0u32;
        for &b in candidates.iter().take(take) {
            let b_pos = self.nodes[b].pos;
            occ.remove(&b_pos);
            let path = routing::route_link(a_pos, b_pos, &occ, bounds, &mut self.rng);
            occ.insert(b_pos);
            if let Some(path) = path {
                self.links.push(Link {
                    a: honey_id,
                    b,
                    path,
                    drawn: 0,
                    kind: LinkKind::Cross,
                    load: 0,
                    breach_ttl: 0,
                });
                revealed += 1;
                self.log_node(b_pos, "backdoor revealed");
            }
        }
        if revealed > 0 {
            let (oa, ob) = octet_pair(a_pos);
            self.push_log(format!(
                "HONEYPOT 10.0.{}.{}: {} backdoors",
                oa, ob, revealed
            ));
        }
    }

    /// When a large cascade just finalized, roll for a "rebirth": one
    /// of the freshly-dead nodes stands back up as the root of a brand-
    /// new faction, with its own C2 entry, faction slot, and faction
    /// stats. The old links and subtree stay dead — it literally rises
    /// from the ashes, parentless, ready to grow its own colony.
    fn maybe_resurrect_c2(&mut self, newly_dead: &[NodeId]) {
        let threshold = self.cfg.resurrection_threshold as usize;
        if threshold == 0 || newly_dead.len() < threshold {
            return;
        }
        if self.cfg.resurrection_chance <= 0.0
            || !self.rng.gen_bool(self.cfg.resurrection_chance as f64)
        {
            return;
        }
        let idx = self.rng.gen_range(0..newly_dead.len());
        let reborn = newly_dead[idx];
        let new_faction = self.c2_nodes.len() as u8;
        let new_branch = self.alloc_branch_id();
        let pos = self.nodes[reborn].pos;
        // Reset the node to a fresh C2 state. Old inbound/outbound
        // links stay as ghost wires on the dead subtree.
        let node = &mut self.nodes[reborn];
        node.state = State::Alive;
        node.dying_in = 0;
        node.parent = None;
        node.born = self.tick;
        node.role = Role::Relay;
        node.hardened = false;
        node.heartbeats = 0;
        node.pulse = 6;
        node.infection = None;
        node.faction = new_faction;
        node.branch_id = new_branch;
        node.pwn_resist = 0;
        node.shield_flash = 0;
        node.mutated_flash = 0;
        node.scan_pulse = 0;
        // Register the new C2 and faction.
        self.c2_nodes.push(reborn);
        self.faction_stats.push(FactionStats::default());
        let (a, b) = octet_pair(pos);
        self.push_log(format!(
            "✦ MYTHIC ✦ REBIRTH — c2[{}] rises @ 10.0.{}.{}",
            new_faction, a, b
        ));
    }

    pub fn schedule_subtree_death(&mut self, root: NodeId, mult: f32) {
        let cascade = self.compute_cascade(root);
        let mut touched = 0u32;
        for (id, distance) in cascade {
            let base = distance.saturating_mul(2).saturating_add(3) as f32;
            let delay = (base * mult).round().clamp(1.0, 255.0) as u8;
            if self.nodes[id].dying_in == 0 || self.nodes[id].dying_in > delay {
                self.nodes[id].dying_in = delay;
                touched += 1;
            }
        }
        if touched > 0 {
            let label = if mult > 1.5 { "HONEYPOT cascade" } else { "cascade" };
            self.push_log(format!("{}: {} hosts burning", label, touched));
            if (touched as usize) >= self.cfg.mythic_big_one_threshold {
                self.push_log(format!("✦ MYTHIC ✦ THE BIG ONE — {} hosts", touched));
            }
            let root_pos = self.nodes[root].pos;
            self.emit_cascade_effects(root_pos, touched);
        }
    }

    fn advance_dying(&mut self) {
        let mut newly_dead: Vec<NodeId> = Vec::new();
        for (i, n) in self.nodes.iter_mut().enumerate() {
            if n.dying_in > 0 {
                n.dying_in -= 1;
                if n.dying_in == 0 && !matches!(n.state, State::Dead) {
                    newly_dead.push(i);
                }
            }
        }
        if newly_dead.is_empty() {
            return;
        }
        for id in &newly_dead {
            let faction = self.nodes[*id].faction;
            self.nodes[*id].state = State::Dead;
            if let Some(s) = self.faction_stats.get_mut(faction as usize) {
                s.lost += 1;
            }
        }
        // When a big batch of nodes dies in one breath, one of them may
        // rise again as the root of a new faction — a new C2 born from
        // the ashes of the fallen subtree.
        self.maybe_resurrect_c2(&newly_dead);
        // Free cells of links that now touch a Dead endpoint so territory reopens.
        let dead: HashSet<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                if matches!(n.state, State::Dead) {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();
        let c2_positions: HashSet<(i16, i16)> =
            self.c2_nodes.iter().map(|&id| self.nodes[id].pos).collect();
        for link in &self.links {
            if dead.contains(&link.a) || dead.contains(&link.b) {
                for c in &link.path {
                    if !c2_positions.contains(c) {
                        self.occupied.remove(c);
                    }
                }
            }
        }
        // Re-seat alive node cells in case we just removed one.
        for n in &self.nodes {
            if !matches!(n.state, State::Dead) {
                self.occupied.insert(n.pos);
            }
        }
    }

    fn push_log(&mut self, s: String) {
        // If the most recent line is identical, bump its repeat count
        // instead of appending a duplicate — consecutive identical
        // events collapse to 'line (×N)' in the rendered log.
        if let Some((last, count)) = self.logs.back_mut() {
            if *last == s {
                *count += 1;
                return;
            }
        }
        self.logs.push_back((s, 1));
        while self.logs.len() > self.cfg.log_cap {
            self.logs.pop_front();
        }
    }

    /// Convenience: log `"node 10.0.X.Y {suffix}"` for events anchored on a
    /// specific mesh position. Used by all simple per-node event log lines.
    fn log_node(&mut self, pos: (i16, i16), suffix: &str) {
        let (a, b) = octet_pair(pos);
        self.push_log(format!("node 10.0.{}.{} {}", a, b, suffix));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduled_subtree_death_eventually_kills_all_descendants() {
        let mut w = World::new(1, (80, 30), Config::default());
        // Kill the RNG-driven loss/spawn so only our scheduled death runs.
        w.cfg.p_spawn = 0.0;
        w.cfg.p_loss = 0.0;
        // Manually build a 3-level tree: c2 -> a -> b -> c
        let a = w.nodes.len();
        w.nodes
            .push(Node::fresh((10, 10), Some(w.c2()), 0, Role::Relay, 1));
        let b = w.nodes.len();
        w.nodes.push(Node::fresh((12, 10), Some(a), 0, Role::Relay, 1));
        let c = w.nodes.len();
        w.nodes.push(Node::fresh((14, 10), Some(b), 0, Role::Relay, 1));
        w.schedule_subtree_death(a, 1.0);
        // All three descendants should be flagged dying but not yet Dead.
        assert!(w.nodes[a].dying_in > 0);
        assert!(w.nodes[b].dying_in > 0);
        assert!(w.nodes[c].dying_in > 0);
        assert!(matches!(w.nodes[a].state, State::Alive));
        // Run enough ticks to drain the deepest dying_in (distance 2 → delay 7).
        for _ in 0..20 {
            w.tick((80, 30));
        }
        assert!(matches!(w.nodes[a].state, State::Dead));
        assert!(matches!(w.nodes[b].state, State::Dead));
        assert!(matches!(w.nodes[c].state, State::Dead));
        assert!(matches!(w.nodes[w.c2()].state, State::Alive));
    }

    #[test]
    fn hardened_node_resists_first_pwn() {
        let mut w = World::new(7, (80, 30), Config::default());
        w.cfg.p_spawn = 0.0;
        let id = w.nodes.len();
        let mut n = Node::fresh((10, 10), Some(w.c2()), 0, Role::Relay, 1);
        n.hardened = true;
        w.nodes.push(n);
        w.cfg.p_loss = 1.0; // force the victim roll to fire
        w.advance_pwned_and_loss();
        assert!(matches!(w.nodes[id].state, State::Alive));
        assert!(!w.nodes[id].hardened);
    }

    #[test]
    fn branch_id_inherits_from_parent_not_c2() {
        let mut w = World::new(11, (120, 40), Config::default());
        // First-hop child gets fresh branch id.
        let a = w.alloc_branch_id();
        w.nodes
            .push(Node::fresh((30, 10), Some(w.c2()), 0, Role::Relay, a));
        let a_id = w.nodes.len() - 1;
        w.nodes
            .push(Node::fresh((32, 10), Some(a_id), 0, Role::Relay, w.nodes[a_id].branch_id));
        assert_ne!(w.nodes[a_id].branch_id, 0);
        assert_eq!(w.nodes[a_id + 1].branch_id, w.nodes[a_id].branch_id);
    }

    #[test]
    fn packet_reaches_c2_and_drops() {
        let mut w = World::new(3, (80, 30), Config::default());
        w.cfg.p_spawn = 0.0;
        w.cfg.p_loss = 0.0;
        // Build chain c2 -> a -> b (exfil)
        let a = w.nodes.len();
        w.nodes
            .push(Node::fresh((10, 10), Some(w.c2()), 0, Role::Relay, 1));
        let b = w.nodes.len();
        w.nodes
            .push(Node::fresh((14, 10), Some(a), 0, Role::Exfil, 1));
        // Manufacture links with full paths marked drawn.
        let path_ca: Vec<(i16, i16)> =
            (w.nodes[w.c2()].pos.0..=10).map(|x| (x, 10)).collect();
        let len_ca = path_ca.len() as u16;
        w.links.push(Link {
            a: w.c2(),
            b: a,
            path: path_ca,
            drawn: len_ca,
            kind: LinkKind::Parent,
            load: 0,
            breach_ttl: 0,
        });
        let path_ab: Vec<(i16, i16)> = (10..=14).map(|x| (x, 10)).collect();
        let len_ab = path_ab.len() as u16;
        w.links.push(Link {
            a,
            b,
            path: path_ab,
            drawn: len_ab,
            kind: LinkKind::Parent,
            load: 0,
            breach_ttl: 0,
        });
        // Force the Exfil to fire on tick 0 and then tick enough for the
        // packet to reach C2 and be dropped.
        w.nodes[b].role_cooldown = 0;
        w.fire_exfil_packets();
        assert_eq!(w.packets.len(), 1);
        for _ in 0..40 {
            w.advance_packets();
        }
        assert!(w.packets.is_empty());
    }

    #[test]
    fn cross_link_saves_reachable_node_from_cascade() {
        let mut w = World::new(5, (80, 30), Config::default());
        w.cfg.p_spawn = 0.0;
        w.cfg.p_loss = 0.0;
        // Diamond: c2 -> a, c2 -> c, a -> b (b in branch_id 1), plus cross b↔c.
        // Kill a. b should die (loses its parent route and isn't cross-linked
        // to anything alive besides c). c is in branch 2 with cross to b, but
        // c has its own parent path to C2, so c must survive. b has no direct
        // parent chain to c after a is gone, so reachability from C2 to b
        // goes c2→c→(cross)→b — b SHOULD survive via the cross.
        let a = w.nodes.len();
        w.nodes
            .push(Node::fresh((20, 10), Some(w.c2()), 0, Role::Relay, 1));
        let c = w.nodes.len();
        w.nodes
            .push(Node::fresh((30, 10), Some(w.c2()), 0, Role::Relay, 2));
        let b = w.nodes.len();
        w.nodes.push(Node::fresh((25, 12), Some(a), 0, Role::Relay, 1));
        // Fully-drawn cross link b ↔ c.
        let cross_path = vec![(25, 12), (30, 10)]; // cells don't matter for logic
        let len = cross_path.len() as u16;
        w.links.push(Link {
            a: b,
            b: c,
            path: cross_path,
            drawn: len,
            kind: LinkKind::Cross,
            load: 0,
            breach_ttl: 0,
        });
        let cascade = w.compute_cascade(a);
        let ids: HashSet<NodeId> = cascade.iter().map(|(id, _)| *id).collect();
        assert!(ids.contains(&a), "root must be doomed");
        assert!(!ids.contains(&b), "b should survive via cross link to c");
        assert!(!ids.contains(&c), "c has its own route to C2");
    }

    #[test]
    fn shield_flash_is_set_when_hardened_node_is_hit() {
        let mut w = World::new(9, (80, 30), Config::default());
        w.cfg.p_spawn = 0.0;
        w.cfg.p_loss = 1.0;
        let id = w.nodes.len();
        let mut n = Node::fresh((10, 10), Some(w.c2()), 0, Role::Relay, 1);
        n.hardened = true;
        w.nodes.push(n);
        w.advance_pwned_and_loss();
        assert!(matches!(w.nodes[id].state, State::Alive));
        assert!(!w.nodes[id].hardened);
        assert!(w.nodes[id].shield_flash > 0, "shield flash should be set");
        // The flash should drain over subsequent ticks.
        w.cfg.p_loss = 0.0; // don't hit it again
        for _ in 0..10 {
            w.tick((80, 30));
        }
        assert_eq!(w.nodes[id].shield_flash, 0);
    }

    #[test]
    fn reconnect_creates_cross_link_between_branches() {
        let mut w = World::new(13, (80, 30), Config::default());
        w.cfg.p_spawn = 0.0;
        w.cfg.p_loss = 0.0;
        w.cfg.reconnect_rate = 1.0;
        w.cfg.reconnect_radius = 20;
        // Two alive nodes in different branches, no existing bridge.
        w.nodes
            .push(Node::fresh((20, 10), Some(w.c2()), 0, Role::Relay, 1));
        w.nodes
            .push(Node::fresh((25, 12), Some(w.c2()), 0, Role::Relay, 2));
        let before = w.links.iter().filter(|l| l.kind == LinkKind::Cross).count();
        w.maybe_reconnect();
        let after = w.links.iter().filter(|l| l.kind == LinkKind::Cross).count();
        assert_eq!(after, before + 1, "should have formed exactly one cross link");
        // Second call should not create a duplicate between the same pair.
        w.maybe_reconnect();
        let cross_count = w.links.iter().filter(|l| l.kind == LinkKind::Cross).count();
        assert_eq!(cross_count, after, "must not duplicate existing bridge");
    }

    #[test]
    fn reconnect_refuses_same_branch() {
        let mut w = World::new(17, (80, 30), Config::default());
        w.cfg.p_spawn = 0.0;
        w.cfg.p_loss = 0.0;
        w.cfg.reconnect_rate = 1.0;
        w.cfg.reconnect_radius = 20;
        // Both nodes in the same branch — should NOT form a cross link.
        w.nodes
            .push(Node::fresh((20, 10), Some(w.c2()), 0, Role::Relay, 1));
        w.nodes
            .push(Node::fresh((25, 12), Some(w.c2()), 0, Role::Relay, 1));
        for _ in 0..20 {
            w.maybe_reconnect();
        }
        let cross = w.links.iter().filter(|l| l.kind == LinkKind::Cross).count();
        assert_eq!(cross, 0);
    }

    #[test]
    fn infection_spreads_along_parent_edges() {
        let mut w = World::new(21, (80, 30), Config::default());
        w.cfg.p_spawn = 0.0;
        w.cfg.p_loss = 0.0;
        w.cfg.virus_seed_rate = 0.0;
        w.cfg.virus_spread_rate = 1.0;
        // Build c2 -> a -> b, infect a and drive it straight to Active so it
        // can infect neighbors.
        let a = w.nodes.len();
        w.nodes
            .push(Node::fresh((10, 10), Some(w.c2()), 0, Role::Relay, 1));
        let b = w.nodes.len();
        w.nodes.push(Node::fresh((12, 10), Some(a), 0, Role::Relay, 1));
        w.nodes[a].infection = Some(Infection {
            strain: 3,
            stage: InfectionStage::Active,
            age: w.cfg.virus_incubation_ticks,
            cure_resist: 3,
            terminal_ticks: 0,
        });
        // Run a few ticks: spread probability is 1.0 so b should catch it fast.
        for _ in 0..5 {
            w.tick((80, 30));
        }
        assert!(w.nodes[b].infection.is_some());
        assert_eq!(w.nodes[b].infection.unwrap().strain, 3);
    }

    #[test]
    fn infection_skips_c2() {
        let mut w = World::new(22, (80, 30), Config::default());
        w.cfg.p_spawn = 0.0;
        w.cfg.p_loss = 0.0;
        w.cfg.virus_seed_rate = 0.0;
        w.cfg.virus_spread_rate = 1.0;
        // Child directly attached to C2, infected and Active.
        let a = w.nodes.len();
        w.nodes
            .push(Node::fresh((10, 10), Some(w.c2()), 0, Role::Relay, 1));
        w.nodes[a].infection = Some(Infection {
            strain: 0,
            stage: InfectionStage::Active,
            age: w.cfg.virus_incubation_ticks,
            cure_resist: 3,
            terminal_ticks: 0,
        });
        for _ in 0..20 {
            w.tick((80, 30));
        }
        assert!(w.nodes[w.c2()].infection.is_none(), "C2 must stay clean");
    }

    #[test]
    fn patch_wave_cures_infected_node_within_radius() {
        let mut w = World::new(24, (80, 30), Config::default());
        w.cfg.p_spawn = 0.0;
        w.cfg.p_loss = 0.0;
        w.cfg.virus_seed_rate = 0.0;
        w.cfg.virus_spread_rate = 0.0;
        w.cfg.worm_spawn_rate = 0.0;
        // Infected node with cure_resist=1, three cells from C2.
        let c2_pos = w.nodes[w.c2()].pos;
        let a = w.nodes.len();
        w.nodes.push(Node::fresh(
            (c2_pos.0 + 3, c2_pos.1),
            Some(w.c2()),
            0,
            Role::Relay,
            1,
        ));
        w.nodes[a].infection = Some(Infection {
            strain: 0,
            stage: InfectionStage::Incubating,
            age: 0,
            cure_resist: 1,
            terminal_ticks: 0,
        });
        // Seed a patch wave directly and tick it forward until the front hits.
        w.patch_waves.push(PatchWave {
            origin: c2_pos,
            radius: 0,
        });
        for _ in 0..10 {
            w.advance_patch_waves();
            if w.nodes[a].infection.is_none() {
                break;
            }
        }
        assert!(w.nodes[a].infection.is_none(), "patch wave should cure the node");
    }

    #[test]
    fn worm_delivered_to_alive_neighbor() {
        let mut w = World::new(25, (80, 30), Config::default());
        w.cfg.p_spawn = 0.0;
        w.cfg.p_loss = 0.0;
        w.cfg.virus_seed_rate = 0.0;
        w.cfg.virus_spread_rate = 0.0;
        // Build c2 -> a -> b with fully-drawn links.
        let a = w.nodes.len();
        w.nodes
            .push(Node::fresh((10, 10), Some(w.c2()), 0, Role::Relay, 1));
        let b = w.nodes.len();
        w.nodes.push(Node::fresh((14, 10), Some(a), 0, Role::Relay, 1));
        let path_ab: Vec<(i16, i16)> = (10..=14).map(|x| (x, 10)).collect();
        let len_ab = path_ab.len() as u16;
        w.links.push(Link {
            a,
            b,
            path: path_ab,
            drawn: len_ab,
            kind: LinkKind::Parent,
            load: 0,
            breach_ttl: 0,
        });
        // Launch a worm from a → b manually and tick the worm advance step
        // enough times for it to reach the far end.
        w.worms.push(Worm {
            link_id: 0,
            pos: 0,
            outbound_from_a: true,
            strain: 2,
        });
        for _ in 0..10 {
            w.advance_worms();
        }
        assert!(w.nodes[b].infection.is_some());
        assert_eq!(w.nodes[b].infection.unwrap().strain, 2);
        assert!(w.worms.is_empty());
    }

    #[test]
    fn terminal_infection_forces_loss() {
        let mut w = World::new(23, (80, 30), Config::default());
        w.cfg.p_spawn = 0.0;
        w.cfg.p_loss = 0.0;
        w.cfg.virus_seed_rate = 0.0;
        w.cfg.virus_spread_rate = 0.0;
        let a = w.nodes.len();
        w.nodes
            .push(Node::fresh((10, 10), Some(w.c2()), 0, Role::Relay, 1));
        w.nodes[a].infection = Some(Infection {
            strain: 0,
            stage: InfectionStage::Terminal,
            age: 200,
            cure_resist: 3,
            terminal_ticks: 1,
        });
        // One tick drains terminal_ticks and flips to Pwned.
        w.tick((80, 30));
        assert!(matches!(
            w.nodes[a].state,
            State::Pwned { .. } | State::Dead
        ));
        assert!(w.nodes[a].infection.is_none());
    }

    #[test]
    fn mutation_skips_honeypots() {
        let mut w = World::new(26, (80, 30), Config::default());
        w.cfg.p_spawn = 0.0;
        w.cfg.p_loss = 0.0;
        w.cfg.mutate_rate = 1.0;
        w.cfg.mutate_min_age = 0;
        w.cfg.virus_seed_rate = 0.0;
        let id = w.nodes.len();
        w.nodes
            .push(Node::fresh((10, 10), Some(w.c2()), 0, Role::Honeypot, 1));
        for _ in 0..10 {
            w.maybe_mutate();
        }
        assert_eq!(w.nodes[id].role, Role::Honeypot);
        assert_eq!(w.nodes[id].mutated_flash, 0);
    }

    #[test]
    fn mutation_flips_relay_role_and_flashes() {
        let mut w = World::new(27, (80, 30), Config::default());
        w.cfg.p_spawn = 0.0;
        w.cfg.p_loss = 0.0;
        w.cfg.mutate_rate = 1.0;
        w.cfg.mutate_min_age = 0;
        w.cfg.virus_seed_rate = 0.0;
        let id = w.nodes.len();
        w.nodes
            .push(Node::fresh((10, 10), Some(w.c2()), 0, Role::Relay, 1));
        w.maybe_mutate();
        assert!(matches!(w.nodes[id].role, Role::Scanner | Role::Exfil));
        assert!(w.nodes[id].mutated_flash > 0);
    }

    #[test]
    fn zero_day_respects_min_node_floor() {
        let mut w = World::new(28, (80, 30), Config::default());
        w.cfg.zero_day_period = 1;
        w.cfg.zero_day_chance = 1.0;
        w.cfg.virus_seed_rate = 0.0;
        // Only C2 alive: 1 node, well below the 10-node minimum.
        w.tick = 1;
        w.maybe_zero_day();
        assert!(w.nodes.iter().all(|n| n.infection.is_none()));
    }

    #[test]
    fn zero_day_outbreak_picks_distinct_targets() {
        let mut w = World::new(31, (80, 30), Config::default());
        w.cfg.p_spawn = 0.0;
        w.cfg.p_loss = 0.0;
        w.cfg.virus_seed_rate = 0.0;
        // Push exactly 4 alive candidates so picking 3-5 distinct should
        // saturate the candidate set without ever double-picking.
        for i in 0..4 {
            w.nodes
                .push(Node::fresh((10 + i, 10), Some(w.c2()), 0, Role::Relay, 1));
        }
        w.zero_day_outbreak();
        let infected = w
            .nodes
            .iter()
            .filter(|n| n.infection.is_some())
            .count();
        // Must hit at least 3 (the configured min) and never exceed 4 (the
        // candidate ceiling). Pre-fix this could come back as 1-2 if the
        // RNG happened to pick duplicates.
        assert!(
            (3..=4).contains(&infected),
            "expected 3-4 distinct infections, got {}",
            infected
        );
    }

    #[test]
    fn infection_spread_skips_honeypots() {
        let mut w = World::new(32, (80, 30), Config::default());
        w.cfg.p_spawn = 0.0;
        w.cfg.p_loss = 0.0;
        w.cfg.virus_seed_rate = 0.0;
        w.cfg.virus_spread_rate = 1.0;
        // Active-infected relay next to a honeypot. Spread fires every tick
        // but the honeypot must remain clean to keep its disguise.
        let infected = w.nodes.len();
        w.nodes
            .push(Node::fresh((10, 10), Some(w.c2()), 0, Role::Relay, 1));
        let honey = w.nodes.len();
        w.nodes
            .push(Node::fresh((12, 10), Some(infected), 0, Role::Honeypot, 1));
        w.nodes[infected].infection = Some(Infection {
            strain: 0,
            stage: InfectionStage::Active,
            age: w.cfg.virus_incubation_ticks,
            cure_resist: 4,
            terminal_ticks: 0,
        });
        for _ in 0..20 {
            w.tick((80, 30));
        }
        assert!(w.nodes[honey].infection.is_none(), "honeypot must stay clean");
    }

    #[test]
    fn defender_pulse_cures_nearby_infection() {
        let mut w = World::new(33, (80, 30), Config::default());
        w.cfg.p_spawn = 0.0;
        w.cfg.p_loss = 0.0;
        w.cfg.virus_seed_rate = 0.0;
        w.cfg.virus_spread_rate = 0.0;
        w.cfg.defender_pulse_period = 1; // fire on every tick
        w.cfg.defender_radius = 5;
        w.cfg.virus_cure_resist = 1; // single-pulse cure
        let _defender = w.nodes.len();
        w.nodes
            .push(Node::fresh((10, 10), Some(w.c2()), 0, Role::Defender, 1));
        let victim = w.nodes.len();
        w.nodes
            .push(Node::fresh((12, 11), Some(w.c2()), 0, Role::Relay, 1));
        w.nodes[victim].infection = Some(Infection {
            strain: 0,
            stage: InfectionStage::Active,
            age: w.cfg.virus_incubation_ticks,
            cure_resist: 1,
            terminal_ticks: 0,
        });
        w.fire_defender_pulses();
        assert!(w.nodes[victim].infection.is_none(), "defender should clear infection in radius");
    }

    #[test]
    fn defender_immune_to_infection() {
        let mut w = World::new(34, (80, 30), Config::default());
        w.cfg.p_spawn = 0.0;
        w.cfg.p_loss = 0.0;
        w.cfg.virus_seed_rate = 0.0;
        w.cfg.virus_spread_rate = 1.0;
        let infected = w.nodes.len();
        w.nodes
            .push(Node::fresh((10, 10), Some(w.c2()), 0, Role::Relay, 1));
        let defender = w.nodes.len();
        w.nodes
            .push(Node::fresh((12, 10), Some(infected), 0, Role::Defender, 1));
        w.nodes[infected].infection = Some(Infection {
            strain: 0,
            stage: InfectionStage::Active,
            age: w.cfg.virus_incubation_ticks,
            cure_resist: 4,
            terminal_ticks: 0,
        });
        for _ in 0..20 {
            w.tick((80, 30));
        }
        assert!(w.nodes[defender].infection.is_none(), "defender should never get infected");
    }

    #[test]
    fn multiple_c2s_each_get_distinct_factions() {
        let cfg = Config {
            c2_count: 3,
            p_spawn: 0.0,
            ..Config::default()
        };
        let w = World::new(40, (120, 30), cfg);
        assert_eq!(w.c2_nodes.len(), 3);
        assert_eq!(w.nodes[w.c2_nodes[0]].faction, 0);
        assert_eq!(w.nodes[w.c2_nodes[1]].faction, 1);
        assert_eq!(w.nodes[w.c2_nodes[2]].faction, 2);
        // Random placement with minimum spacing — every pair must be
        // at a distinct cell.
        for i in 0..w.c2_nodes.len() {
            for j in (i + 1)..w.c2_nodes.len() {
                assert_ne!(
                    w.nodes[w.c2_nodes[i]].pos,
                    w.nodes[w.c2_nodes[j]].pos,
                    "C2s {} and {} overlap",
                    i,
                    j
                );
            }
        }
    }

    #[test]
    fn cascade_does_not_kill_other_factions() {
        let cfg = Config {
            c2_count: 2,
            p_spawn: 0.0,
            p_loss: 0.0,
            virus_seed_rate: 0.0,
            ..Config::default()
        };
        let mut w = World::new(41, (80, 30), cfg);
        // Build one child for each faction.
        let f0 = w.c2_nodes[0];
        let f1 = w.c2_nodes[1];
        let child0 = w.nodes.len();
        let mut n0 = Node::fresh((10, 10), Some(f0), 0, Role::Relay, 1);
        n0.faction = 0;
        w.nodes.push(n0);
        let child1 = w.nodes.len();
        let mut n1 = Node::fresh((40, 10), Some(f1), 0, Role::Relay, 2);
        n1.faction = 1;
        w.nodes.push(n1);
        // Trigger a cascade on faction 0's child. Faction 1 must survive.
        w.schedule_subtree_death(child0, 1.0);
        for _ in 0..20 {
            w.tick((80, 30));
        }
        assert!(matches!(w.nodes[child0].state, State::Dead));
        assert!(matches!(w.nodes[child1].state, State::Alive));
        assert!(matches!(w.nodes[f1].state, State::Alive));
    }

    #[test]
    fn day_night_cycle_flips_at_half_period() {
        let cfg = Config {
            day_night_period: 100,
            ..Config::default()
        };
        let mut w = World::new(50, (80, 30), cfg);
        assert!(!w.is_night(), "starts in day phase");
        w.tick = 49;
        assert!(!w.is_night(), "still day just before midpoint");
        w.tick = 50;
        assert!(w.is_night(), "night at midpoint");
        w.tick = 99;
        assert!(w.is_night(), "still night at period end");
        w.tick = 100;
        assert!(!w.is_night(), "day at next period start");
    }

    #[test]
    fn link_load_accumulates_and_decays() {
        let cfg = Config {
            p_spawn: 0.0,
            p_loss: 0.0,
            virus_seed_rate: 0.0,
            ..Config::default()
        };
        let mut w = World::new(60, (80, 30), cfg);
        let a = w.nodes.len();
        w.nodes
            .push(Node::fresh((10, 10), Some(w.c2()), 0, Role::Relay, 1));
        let b = w.nodes.len();
        w.nodes.push(Node::fresh((14, 10), Some(a), 0, Role::Exfil, 1));
        let path: Vec<(i16, i16)> = (10..=14).map(|x| (x, 10)).collect();
        let len = path.len() as u16;
        w.links.push(Link {
            a,
            b,
            path,
            drawn: len,
            kind: LinkKind::Parent,
            load: 0,
            breach_ttl: 0,
        });
        // Park a packet on the link and tick the motion phase a few times.
        w.packets.push(Packet {
            link_id: 0,
            pos: len - 1,
        });
        for _ in 0..5 {
            w.decay_link_load();
            w.advance_packets();
        }
        assert!(w.links[0].load > 0, "load should accumulate from in-flight packet");

        // Stop feeding packets; load decays back to zero.
        w.packets.clear();
        for _ in 0..20 {
            w.decay_link_load();
        }
        assert_eq!(w.links[0].load, 0, "load should decay to zero");
    }

    #[test]
    fn honeypot_trip_reveals_backdoor_links() {
        let cfg = Config {
            p_spawn: 0.0,
            p_loss: 0.0,
            virus_seed_rate: 0.0,
            honeypot_backdoor_max: 3,
            honeypot_backdoor_radius: 20,
            ..Config::default()
        };
        let mut w = World::new(70, (80, 30), cfg);
        // Honeypot in its own branch, plus three alive neighbors in
        // separate branches within the backdoor radius.
        let honey = w.nodes.len();
        let mut h = Node::fresh((20, 10), Some(w.c2()), 0, Role::Honeypot, 1);
        h.faction = 0;
        w.nodes.push(h);
        for (i, pos) in [(25, 10), (18, 15), (22, 12)].iter().enumerate() {
            let mut n = Node::fresh(*pos, Some(w.c2()), 0, Role::Relay, 2 + i as u16);
            n.faction = 0;
            w.nodes.push(n);
        }
        let before = w
            .links
            .iter()
            .filter(|l| l.kind == LinkKind::Cross)
            .count();
        w.reveal_honeypot_backdoors(honey);
        let after = w
            .links
            .iter()
            .filter(|l| l.kind == LinkKind::Cross)
            .count();
        assert!(
            after > before,
            "expected at least one backdoor cross-link to be added; before={} after={}",
            before,
            after
        );
        // All new cross-links should originate from the honeypot.
        let from_honey = w
            .links
            .iter()
            .filter(|l| l.kind == LinkKind::Cross && l.a == honey)
            .count();
        assert_eq!(
            from_honey,
            after - before,
            "all revealed backdoors should anchor on the honeypot"
        );
    }

    #[test]
    fn tower_absorbs_pwn_attempts_before_dying() {
        let cfg = Config {
            p_spawn: 0.0,
            p_loss: 1.0,
            virus_seed_rate: 0.0,
            tower_pwn_resist: 2,
            ..Config::default()
        };
        let mut w = World::new(80, (80, 30), cfg);
        // One tower adjacent to C2 — the only possible victim.
        let tower = w.nodes.len();
        let mut n = Node::fresh((11, 10), Some(w.c2()), 0, Role::Tower, 1);
        n.pwn_resist = 2;
        n.faction = 0;
        w.nodes.push(n);
        // Three ticks of guaranteed loss rolls. First two should consume
        // the pwn_resist charges; third should finally pwn the tower.
        for _ in 0..2 {
            w.tick((80, 30));
            assert!(
                matches!(w.nodes[tower].state, State::Alive),
                "tower should still be alive"
            );
        }
        w.tick((80, 30));
        assert!(
            matches!(w.nodes[tower].state, State::Pwned { .. } | State::Dead),
            "tower should be down after 3 hits"
        );
    }

    #[test]
    fn c2_count_randomized_within_range() {
        // With c2_count=2 and c2_count_max=4, every seed should land
        // somewhere in 2..=4, but different seeds hit different counts.
        let mut counts = std::collections::HashSet::new();
        for seed in 0..50u64 {
            let cfg = Config {
                c2_count: 2,
                c2_count_max: 4,
                p_spawn: 0.0,
                ..Config::default()
            };
            let w = World::new(seed, (120, 30), cfg);
            let n = w.c2_nodes.len();
            assert!((2..=4).contains(&n), "seed {} gave {} c2s", seed, n);
            counts.insert(n);
        }
        // With 50 seeds, we should see at least 2 distinct counts.
        assert!(counts.len() >= 2, "expected varied counts, got {:?}", counts);
    }

    #[test]
    fn large_cascade_can_resurrect_a_new_c2() {
        let cfg = Config {
            p_spawn: 0.0,
            p_loss: 0.0,
            virus_seed_rate: 0.0,
            resurrection_threshold: 3,
            resurrection_chance: 1.0,
            ..Config::default()
        };
        let mut w = World::new(90, (80, 30), cfg);
        // Manually stage three Alive nodes with dying_in == 1 so they
        // all die on the next advance_dying tick, triggering the
        // resurrection roll (threshold=3, chance=1.0 = guaranteed).
        for i in 0..3 {
            let id = w.nodes.len();
            let mut n = Node::fresh((10 + i, 10), Some(w.c2()), 0, Role::Relay, 1);
            n.faction = 0;
            n.dying_in = 1;
            w.nodes.push(n);
            let _ = id;
        }
        let before = w.c2_nodes.len();
        w.advance_dying();
        let after = w.c2_nodes.len();
        assert_eq!(
            after,
            before + 1,
            "expected a rebirth to add one C2; before={} after={}",
            before,
            after
        );
        // The resurrected C2 should be parentless and alive.
        let new_c2 = *w.c2_nodes.last().unwrap();
        assert!(matches!(w.nodes[new_c2].state, State::Alive));
        assert!(w.nodes[new_c2].parent.is_none());
    }

    #[test]
    fn tick_runs_without_panic_and_grows() {
        let mut w = World::new(42, (80, 24), Config::default());
        for _ in 0..500 {
            w.tick((80, 24));
        }
        assert!(w.nodes.len() > 1);
    }
}
