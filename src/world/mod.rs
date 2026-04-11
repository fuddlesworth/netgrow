use std::collections::{HashMap, HashSet, VecDeque};

use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

mod cascade;
mod config;
mod events;
mod packets;
mod roles;
mod spawn;
mod types;
mod virus;
pub use config::*;
pub use spawn::node_ip;
use spawn::octet_pair;
pub use types::*;

/// Number of distinct virus strains. Used by strain-indexed palettes in
/// render and by the modular wraparound in spread-tally logic.
pub const STRAIN_COUNT: usize = 8;

/// Worms advance one cell every N sim ticks so each cell stays on-screen
/// long enough to see.
const WORM_STEP_INTERVAL: u64 = 2;

/// Link load threshold for the "warm" render tier: accent color with a
/// bold modifier. Below this the link uses its normal branch hue. Also
/// the exfil backpressure threshold — an exfil whose inbound link is
/// warm or hotter skips its emission cycle and retries on a shorter
/// cooldown, so traffic self-throttles before the chain saturates.
/// Kept high enough above a single in-flight packet's contribution
/// that routine traffic isn't instantly throttled.
pub const WARM_LINK: u8 = 10;
/// Link load threshold for the "hot" render tier: cascade color. Packets
/// refuse to hop onto a link whose load has crossed this, and the link's
/// `burn_ticks` counter climbs while it stays above this line.
pub const HOT_LINK: u8 = 16;
/// How much each in-flight packet adds to its current link's load per tick.
const PACKET_LOAD_INCREMENT: u8 = 1;
/// How much each in-flight worm adds to its current link's load per tick.
const WORM_LOAD_INCREMENT: u8 = 1;

/// Sustained-hot ticks that upgrade a link's child endpoint into a
/// Router on a probabilistic roll. The morph bypasses the normal
/// mutation lock — it's the mesh adapting to traffic pressure in
/// place, not a background mutation.
const ROUTER_UPGRADE_THRESHOLD: u8 = 20;
/// Per-tick chance (while over `ROUTER_UPGRADE_THRESHOLD`) that an
/// eligible child endpoint morphs into a Router. Kept relatively high
/// so the response to congestion feels immediate but still organic.
const ROUTER_UPGRADE_CHANCE: f64 = 0.25;
/// Sustained-hot ticks that collapse a link entirely, clearing all
/// traffic, spiking both endpoints' role cooldowns, and quarantining
/// the link for `LINK_QUARANTINE_TICKS`. The rare dramatic response
/// when Router upgrades and cross-link reroutes fail to relieve the
/// pressure in time.
const LINK_COLLAPSE_THRESHOLD: u8 = 60;
/// How long a collapsed link stays unavailable to packets before it
/// can carry traffic again.
const LINK_QUARANTINE_TICKS: u8 = 40;

/// Ticks a freshly-dead node keeps rendering its old role glyph as a
/// dim "ghost echo" before the render pass falls back to the plain
/// dead marker. Makes deaths visible as fading traces instead of
/// instantly clearing.
pub const GHOST_ECHO_TICKS: u16 = 600;
/// Decay thresholds — ghosts cross these as their echo counter
/// ticks down. Above `GHOST_FADE_TICKS` they render as the old
/// role glyph dimmed; below that they render as a faint `·`;
/// at 0 they're cleaned up.
pub const GHOST_FADE_TICKS: u16 = 200;

/// Starting `pwn_resist` reservoir for a C2 node. Enemy worms that
/// cross into a C2's cell drain this pool each time they deliver;
/// when it hits zero the C2 falls and its whole subtree cascades.
pub const C2_INITIAL_HP: u8 = 200;
/// Amount drained from a C2's pwn_resist by each cross-faction worm
/// that successfully delivers to its cell. Tuned so it takes several
/// dozen hostile deliveries to crack a C2.
pub const C2_WORM_DAMAGE: u8 = 8;

/// How many patch-wave survivals an infection needs to absorb before
/// it gets a veteran rank bump and a permanent `cure_resist` bonus.
pub const VETERAN_WAVE_THRESHOLD: u8 = 2;
/// Maximum `cure_resist` a veteran infection can reach via survivals.
/// Caps the escalation so veterans are harder but never immortal.
pub const VETERAN_CURE_RESIST_CAP: u8 = 6;

/// Ticks of post-cure immunity granted to a node when an infection
/// is cleared. Strain-specific — while the window is open, the
/// node can't be re-infected with the same strain via spread or
/// worm delivery, but other strains still land normally.
pub const IMMUNITY_DURATION_TICKS: u16 = 180;

/// Per-tick chance a weaker strain's nodes are outcompeted by the
/// ecosystem's dominant strain. Only fires when there's a >= 2x
/// strength gap, so minor diversity doesn't get squashed.
pub const STRAIN_OUTCOMPETE_CHANCE: f32 = 0.01;

/// Ticks a favoritism boost lasts after a single 1-9 key press.
pub const FAVOR_DURATION_TICKS: u64 = 300;
/// Spawn-weight multiplier applied to the favored faction's parent
/// weight while the favor window is active. 2x roughly doubles
/// its odds of being picked as the next spawn's parent.
pub const FAVOR_WEIGHT_MULT: f32 = 2.5;

/// Minimum age (in ticks) a node needs before it can be promoted to
/// legendary status. Combined with `LEGENDARY_MIN_CHILDREN` to gate
/// the rare long-lived, reproductive characters.
pub const LEGENDARY_MIN_AGE: u64 = 1200;
/// Minimum number of direct children a node must have spawned to
/// qualify for a legendary name.
pub const LEGENDARY_MIN_CHILDREN: u16 = 8;

/// Maximum value any relation's pressure can hold. Caps the
/// multiplier so even ancient feuds eventually plateau instead of
/// melting events.
pub const RIVALRY_CAP: u16 = 200;

/// Pressure threshold that promotes a Neutral relation into ColdWar.
/// Below this, pressure accumulates quietly — no visible state
/// change; above it, the relation flips to an explicit hostile
/// posture and `advance_diplomacy` starts watching for the war
/// escalation.
pub const COLD_WAR_THRESHOLD: u16 = 60;
/// Pressure threshold that promotes a ColdWar relation into OpenWar.
/// Aggressor-persona factions see a lowered effective threshold via
/// `persona_war_bonus`, so their wars declare earlier.
pub const WAR_DECLARATION_THRESHOLD: u16 = 120;
/// Pressure level that an OpenWar must drop back below before it
/// can de-escalate into ColdWar on timer expiry.
pub const WAR_DE_ESCALATE_THRESHOLD: u16 = 80;

/// Trust cap in both positive and negative directions. Trust
/// accumulates while peaceful states hold and plummets on
/// betrayal (alliance broken, war declared from a peaceful state).
pub const TRUST_CAP: i16 = 100;
/// Trust level at which a Trade relation can upgrade to
/// NonAggression on timer expiry.
pub const NAP_TRUST_THRESHOLD: i16 = 30;
/// Trust level at which a NonAggression relation can upgrade to
/// Alliance on timer expiry.
pub const ALLIANCE_TRUST_THRESHOLD: i16 = 60;

/// State durations in ticks. Timer-bounded states fall back to a
/// follow-up state on expiry; pressure/trust/death-driven states
/// use `expires_tick = 0` to mean "no timer".
pub const STATE_DURATION_TRADE: u64 = 800;
pub const STATE_DURATION_NAP: u64 = 1000;
pub const STATE_DURATION_ALLIANCE: u64 = 600;
pub const STATE_DURATION_COLD_WAR: u64 = 400;
pub const STATE_DURATION_OPEN_WAR: u64 = 500;

/// Research point thresholds for each tier of the persona tech tree.
/// A faction that crosses `TECH_TIER_1_COST` total research unlocks
/// Tier 1, and so on. Costs are cumulative and research never
/// decrements — once unlocked a tier stays unlocked even if the
/// faction then shrinks. Tuned so a typical mid-size faction
/// unlocks T1 around tick 2000, T2 around tick 6000, T3 around
/// tick 15000 — the system is meant to read as long-arc progress,
/// not an early-game power spike.
pub const TECH_TIER_1_COST: u32 = 200;
pub const TECH_TIER_2_COST: u32 = 700;
pub const TECH_TIER_3_COST: u32 = 1800;
/// Per-sample chance that a Tier 3 faction fires its persona's
/// active ability. Low enough that the active fires roughly once
/// every few minutes of real time while still being noticeable in
/// the log feed.
pub const TECH_T3_ACTIVE_CHANCE: f64 = 0.22;

/// Fraction of total alive nodes a single faction needs to hold
/// to be counted as the currently-dominant faction. Fires a log
/// line on transitions, never ends the run — the sim keeps
/// going until the user quits.
pub const VICTORY_ALIVE_FRACTION: f32 = 0.60;

/// Fraction of a faction's peak alive count it must drop to before
/// scorched-earth becomes possible. A faction that has lost at
/// least this much of its peak is eligible to trigger the
/// self-destruct cascade.
pub const SCORCHED_EARTH_TRIGGER_FRACTION: f32 = 0.25;
/// Minimum peak alive count a faction must have reached to even
/// qualify for scorched-earth — prevents tiny factions from
/// instantly self-destructing after their first cascade.
pub const SCORCHED_EARTH_MIN_PEAK: u32 = 20;
/// Per-sample-tick chance that an eligible faction actually fires
/// its scorched-earth protocol once the trigger condition is met.
pub const SCORCHED_EARTH_CHANCE: f64 = 0.4;

/// Number of successful packet deliveries a Parent link must carry
/// before it gets promoted to a backbone link with an inflated HOT
/// ceiling and a thicker glyph.
pub const BACKBONE_PROMOTION_THRESHOLD: u16 = 30;
/// Inflated HOT_LINK ceiling for backbone links. Higher than the
/// regular HOT_LINK so backbones can carry more concurrent traffic
/// before refusing packets.
pub const BACKBONE_HOT_LINK: u8 = 28;

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
    pub ddos_waves: Vec<DdosWave>,
    pub wormholes: Vec<Wormhole>,
    pub next_branch_id: u16,
    /// Tick at which the current network storm ends. 0 if no storm is
    /// active. Storms spike both spawn and loss rates for a short burst.
    pub storm_until: u64,
    /// Tick the current storm started at. Paired with `storm_until`
    /// so the renderer can compute the front's advance along the
    /// storm's direction vector.
    pub storm_since: u64,
    /// Direction the current storm's crackle front is rolling.
    /// Always starts at the top edge and moves downward (dy = 1),
    /// with an optional left or right drift (dx ∈ {-1, 0, 1}).
    pub storm_dir: (i8, i8),
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
    /// Per-faction-pair diplomatic state machine. Keyed by canonical
    /// `(min(a,b), max(a,b))`. Replaces the earlier split between
    /// `rivalry`, `wars`, and `alliances` with a single unified
    /// record per pair that owns the current `DiplomaticState`,
    /// pressure, trust, and the state timer. `advance_diplomacy`
    /// drives transitions on the faction sample cadence; every
    /// call site (`at_war`, `allied`, `rivalry_pressure`,
    /// `bump_rivalry`, render panels) reads through helper methods
    /// that back-compat to the old API.
    pub relations: HashMap<(u8, u8), Relation>,
    /// Active ISP outage zones: rectangular dead regions where new
    /// spawns are blocked and any alive nodes inside take a steady
    /// role-cooldown spike. Spawned by `maybe_isp_outage` and
    /// dissolved by `advance_outages`.
    pub outages: Vec<IspOutage>,
    /// Active network partitions: horizontal or vertical slices
    /// through the mesh. Packets and worms crossing an active
    /// partition drop instantly, and new cross-faction bridges
    /// can't form through one. Companion to IspOutage.
    pub partitions: Vec<Partition>,
    /// Per-faction AI personality. Indexed in lockstep with
    /// `c2_nodes` and `faction_stats`. Picked at World::new and
    /// when a faction is birthed via resurrection. Drives
    /// per-faction role-weight biases in roll_role and a few
    /// event rolls so factions feel distinct.
    pub personas: Vec<Persona>,
    /// Per-faction index into the theme's `faction_palette`.
    /// Indexed in lockstep with `c2_nodes` and shuffled at
    /// `World::new` so each run's factions pick up different
    /// colors from the palette instead of F0 always being hue[0],
    /// F1 always being hue[1], etc.
    pub faction_colors: Vec<usize>,
    /// Faction currently holding dominance (≥ VICTORY_ALIVE_FRACTION
    /// of total alive nodes, or sole surviving C2). Cleared when
    /// the dominant faction drops below the threshold. Purely a
    /// readout — the sim never auto-ends; dominance is just a
    /// tracked state the UI and summary surface.
    pub current_dominant: Option<u8>,
    /// Cumulative count of distinct dominance declarations fired
    /// this run, so the summary can show 'F0 crowned 3 times'.
    pub dominance_shifts: u32,
    /// Fixed fiber-zone terrain rolled at world creation. Nodes
    /// spawned inside a hotspot start with bonus pwn_resist so
    /// factions have territory worth contesting.
    pub hotspots: Vec<Hotspot>,
    /// Per-strain "patent" ownership. When a strain is produced
    /// via hybrid merging, the faction whose node hosted the
    /// merge claims the patent. Every sample period, rival
    /// factions carrying an owned strain pay the owner a trickle
    /// of intel — viral ecology becomes a passive income vector
    /// for the faction that first crossed the strains.
    pub strain_patents: Vec<Option<u8>>,
    /// Currently-favored faction (via cursor-mode 1-8 hotkey).
    /// Boost expires at `favor_expires_tick`; when elapsed, the
    /// field clears and spawn rolls stop biasing.
    pub favored_faction: Option<u8>,
    pub favor_expires_tick: u64,
    /// Active era's mechanical rule set. Recomputed at each epoch
    /// boundary via `era_rules_for`. Individual tick-loop sites read
    /// the relevant multiplier from this struct and fold it into the
    /// existing cfg-derived values.
    pub era_rules: EraRules,
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
            if count > stats.peak_alive {
                stats.peak_alive = count;
            }
        }
        self.activity_history.push_back(total);
        while self.activity_history.len() > ACTIVITY_HISTORY_LEN {
            self.activity_history.pop_front();
        }
    }

    /// Reactive persona shifts based on current vs peak vs average
    /// alive counts. A faction that's lost most of its peak flips to
    /// Fortress (turtle); a faction running well above the average
    /// flips to Aggressor (expansion). Plague factions hold their
    /// viral identity and never shift. Runs on the same slow cadence
    /// as the faction sampler so flips read as deliberate state
    /// changes, not jitter.
    fn maybe_shift_personas(&mut self) {
        if self.faction_stats.len() < 2 {
            return;
        }
        let counts: Vec<u32> = self
            .faction_stats
            .iter()
            .map(|fs| fs.history.back().copied().unwrap_or(0))
            .collect();
        let total: u32 = counts.iter().sum();
        let avg = total as f32 / counts.len() as f32;
        let mut shifts: Vec<(usize, Persona, Persona)> = Vec::new();
        for (i, fs) in self.faction_stats.iter().enumerate() {
            let cur = counts[i] as f32;
            let peak = fs.peak_alive as f32;
            let Some(persona) = self.personas.get(i).copied() else {
                continue;
            };
            // Plague factions stay Plague — their identity is viral,
            // not state-driven.
            if matches!(persona, Persona::Plague) {
                continue;
            }
            let target = if peak >= 8.0 && cur <= peak * 0.4 {
                Persona::Fortress
            } else if avg >= 4.0 && cur >= avg * 1.5 {
                Persona::Aggressor
            } else {
                Persona::Opportunist
            };
            if target != persona {
                shifts.push((i, persona, target));
            }
        }
        for (i, from, to) in shifts {
            self.personas[i] = to;
            self.push_log(format!(
                "F{} persona shift: {} → {}",
                i,
                from.display_name(),
                to.display_name()
            ));
        }
    }

    /// True if the given faction pair is currently in an active
    /// open-war state, i.e. their rivalry crossed
    /// True if the relation between `a` and `b` is currently in
    /// `OpenWar`. Back-compat helper so every existing caller
    /// (skirmish amp, sleeper lattice, render marks) reads one
    /// predicate instead of inlining a match.
    pub fn at_war(&self, a: u8, b: u8) -> bool {
        matches!(self.relation_state(a, b), DiplomaticState::OpenWar)
    }

    /// Scan rivalry pairs for newly-crossed war declarations and
    /// promote them. A pair only declares once per rivalry lifetime
    /// — after a declaration fires, the rivalry has to fully decay
    /// below the threshold and re-climb to trigger another.
    /// Sleeper-lattice activation — a periodic pass that wakes
    /// dormant cross-links when specific conditions fire:
    /// either endpoint's faction is currently at war with any
    /// other faction, OR one endpoint's parent (if any) is dead
    /// or dying, meaning the node would otherwise lose its chain
    /// to C2 and the sleeper can save it. Activated edges flip
    /// `latent = false` and announce themselves with a
    /// `✦ lattice ✦` log line.
    fn activate_sleeper_lattice(&mut self) {
        let mut to_wake: Vec<usize> = Vec::new();
        for (i, link) in self.links.iter().enumerate() {
            if !link.latent || link.kind != LinkKind::Cross {
                continue;
            }
            let a = &self.nodes[link.a];
            let b = &self.nodes[link.b];
            if !matches!(a.state, State::Alive) || !matches!(b.state, State::Alive) {
                continue;
            }
            // Condition 1: endpoint's faction is at war with
            // anyone. Check both endpoints' factions against
            // every other faction.
            let a_fac = a.faction;
            let b_fac = b.faction;
            let faction_count = self.faction_stats.len() as u8;
            let at_war = (0..faction_count).any(|f| {
                f != a_fac && self.at_war(a_fac, f) || f != b_fac && self.at_war(b_fac, f)
            });
            // Condition 2: either endpoint has a parent that is
            // dead or dying, meaning the node is about to lose
            // its chain.
            let parent_lost = |n: &crate::world::Node| -> bool {
                if let Some(pid) = n.parent {
                    let p = &self.nodes[pid];
                    matches!(p.state, State::Dead) || p.dying_in > 0
                } else {
                    false
                }
            };
            let isolated = parent_lost(a) || parent_lost(b);
            if at_war || isolated {
                to_wake.push(i);
            }
        }
        for li in to_wake {
            self.links[li].latent = false;
            let (a, b) = {
                let link = &self.links[li];
                (link.a, link.b)
            };
            let pos = self.nodes[a].pos;
            let (oa, ob) = octet_pair(pos);
            self.push_log(format!(
                "✦ lattice ✦ sleeper edge wakes @ 10.0.{}.{} (F{}↔F{})",
                oa, ob, self.nodes[a].faction, self.nodes[b].faction
            ));
        }
    }

    /// Strain patent royalties — every sample period, each
    /// faction that owns a patented strain collects +1 intel per
    /// rival-faction host currently carrying that strain. Creates
    /// passive income from viral ecology: mutating a hybrid
    /// creates a long-tail revenue stream as long as the strain
    /// stays circulating among rival factions.
    fn collect_strain_patents(&mut self) {
        // Tally count of rival hosts per (strain, owner) pair.
        let mut royalties: HashMap<u8, u32> = HashMap::new();
        for n in &self.nodes {
            if !matches!(n.state, State::Alive) {
                continue;
            }
            let Some(inf) = n.infection else { continue };
            let Some(owner) = self
                .strain_patents
                .get(inf.strain as usize)
                .and_then(|&o| o)
            else {
                continue;
            };
            if owner == n.faction {
                continue; // own-faction hosts don't pay royalties
            }
            *royalties.entry(owner).or_insert(0) += 1;
        }
        for (owner, amount) in royalties {
            if let Some(s) = self.faction_stats.get_mut(owner as usize) {
                s.intel = s.intel.saturating_add(amount);
            }
        }
    }

    /// Scorched-earth protocol — when a faction has fallen
    /// below `SCORCHED_EARTH_TRIGGER_FRACTION` of its peak,
    /// each sample period rolls a small chance that it defies
    /// assimilation by chain-collapsing its own subtree from
    /// the C2 down. Leaves rubble zones other factions have to
    /// reclaim instead of just walking into. One-shot per
    /// faction lifetime (tracked via
    /// `FactionStats.scorched_earth_fired`).
    fn maybe_scorched_earth(&mut self) {
        let mut eligible: Vec<(u8, NodeId)> = Vec::new();
        for (fid_usize, fs) in self.faction_stats.iter().enumerate() {
            if fs.scorched_earth_fired {
                continue;
            }
            if fs.peak_alive < SCORCHED_EARTH_MIN_PEAK {
                continue;
            }
            let cur = fs.history.back().copied().unwrap_or(0);
            if (cur as f32) > (fs.peak_alive as f32) * SCORCHED_EARTH_TRIGGER_FRACTION {
                continue;
            }
            if cur == 0 {
                // Already wiped out — nothing to burn.
                continue;
            }
            // Find the faction's C2 node (if any still alive).
            let fid = fid_usize as u8;
            let c2 = self
                .c2_nodes
                .iter()
                .copied()
                .find(|&id| {
                    matches!(self.nodes[id].state, State::Alive)
                        && self.nodes[id].faction == fid
                });
            if let Some(c2_id) = c2 {
                eligible.push((fid, c2_id));
            }
        }
        for (fid, c2_id) in eligible {
            if !self.rng.gen_bool(SCORCHED_EARTH_CHANCE) {
                continue;
            }
            if let Some(fs) = self.faction_stats.get_mut(fid as usize) {
                fs.scorched_earth_fired = true;
            }
            let pos = self.nodes[c2_id].pos;
            let (a, b) = octet_pair(pos);
            self.push_log(format!(
                "✦ SCORCHED EARTH ✦ F{} initiates total collapse @ 10.0.{}.{}",
                fid, a, b
            ));
            // Schedule the whole subtree — schedule_subtree_death
            // already walks the cascade reachability, so this
            // burns every node reachable from the C2 within the
            // faction. Use a high honeypot-style multiplier so
            // the rubble lingers longer as ghosts before fully
            // decaying.
            self.schedule_subtree_death(c2_id, 2.0);
            // Also kill the C2 itself explicitly (it's exempt
            // from the cascade reachability that compute_cascade
            // uses, since that anchors on the C2).
            self.nodes[c2_id].dying_in = 1;
        }
    }

    /// Pressure threshold at which `persona_war_bonus` accounts for
    /// Aggressor-persona factions escalating earlier than others.
    fn persona_war_threshold(&self, a: u8, b: u8) -> u16 {
        let aggressor = |p: Option<&Persona>| matches!(p, Some(Persona::Aggressor));
        let pa = self.personas.get(a as usize);
        let pb = self.personas.get(b as usize);
        if aggressor(pa) || aggressor(pb) {
            WAR_DECLARATION_THRESHOLD.saturating_sub(20)
        } else {
            WAR_DECLARATION_THRESHOLD
        }
    }

    /// Trust gain multiplier for a pair based on their personas.
    /// Each side contributes independently (Fortress 2.0×,
    /// Opportunist 1.5×, Aggressor 0.5×, Plague 1.0×) and the
    /// pair's final multiplier is the **average** of the two
    /// contributions. Design implication: mixed pairs still grow
    /// trust net-positive as long as at least one side wants
    /// peace — a Fortress+Aggressor pair gets `(2.0 + 0.5) / 2 =
    /// 1.25×`, which reads as "the Fortress carries the peace
    /// despite the Aggressor's reluctance." If you want an
    /// Aggressor in the pair to hard-block trust growth, switch
    /// this to `pa.min(pb)` — intentional choice, not a bug.
    fn persona_trust_mult(&self, a: u8, b: u8) -> f32 {
        let contribution = |p: Option<&Persona>| -> f32 {
            match p {
                Some(Persona::Fortress) => 2.0,
                Some(Persona::Opportunist) => 1.5,
                Some(Persona::Aggressor) => 0.5,
                _ => 1.0,
            }
        };
        let pa = contribution(self.personas.get(a as usize));
        let pb = contribution(self.personas.get(b as usize));
        (pa + pb) * 0.5
    }

    /// Alive-node count per faction — helper used by Vassalage
    /// dominance checks and by `advance_diplomacy`'s trade rolls.
    fn faction_alive_counts(&self) -> Vec<u32> {
        let mut counts = vec![0u32; self.faction_stats.len()];
        for n in &self.nodes {
            if matches!(n.state, State::Alive) {
                if let Some(slot) = counts.get_mut(n.faction as usize) {
                    *slot += 1;
                }
            }
        }
        counts
    }

    /// Diplomacy state-machine driver. Called once per faction
    /// sample period. Split into three cohesive passes so each
    /// step has one job: `sweep_stale_relations` drops entries
    /// for dead factions, `advance_relation_transitions` runs
    /// the state machine over every surviving pair, and
    /// `maybe_propose_trade` rolls an opportunistic Trade
    /// proposal between a random Neutral pair. Previously the
    /// method was one 250-line block with tangled early-exit
    /// paths that masked a `return`-based control-flow bug.
    fn advance_diplomacy(&mut self) {
        let faction_count = self.c2_nodes.len() as u8;
        if faction_count < 2 {
            return;
        }
        self.sweep_stale_relations();
        self.advance_relation_transitions();
        self.maybe_propose_trade(faction_count);
    }

    /// Drop every relation entry involving a faction whose C2 is
    /// no longer Alive, plus any Vassalage whose overlord has
    /// died. Catches every death route centrally — the cascade
    /// pipeline still runs its own inline purge for cascade
    /// deaths, but assimilation (which flips C2s to Dead directly)
    /// and any future path rely on this safety sweep.
    fn sweep_stale_relations(&mut self) {
        let alive_c2: Vec<bool> = self
            .c2_nodes
            .iter()
            .map(|&id| matches!(self.nodes[id].state, State::Alive))
            .collect();
        // Faction id must equal `c2_nodes` index — asserted in
        // debug builds only, since every call site currently
        // preserves this invariant and the runtime cost matters
        // on the diplomacy hot path.
        debug_assert_eq!(
            alive_c2.len(),
            self.c2_nodes.len(),
            "faction id / c2_nodes index invariant broken"
        );
        let faction_alive = |f: u8| -> bool {
            alive_c2.get(f as usize).copied().unwrap_or(false)
        };
        let before = self.relations.len();
        self.relations.retain(|&(a, b), rel| {
            if !faction_alive(a) || !faction_alive(b) {
                return false;
            }
            if let DiplomaticState::Vassalage { overlord } = rel.state {
                if !faction_alive(overlord) {
                    return false;
                }
            }
            true
        });
        let purged = before.saturating_sub(self.relations.len());
        if purged > 0 {
            self.push_log(format!(
                "diplomacy: {} stale relations severed",
                purged
            ));
        }
    }

    /// Walk every live relation, collect pressure decays, trust
    /// bumps, and state transitions into side buffers, then
    /// apply them. Split from the driver so the state-machine
    /// arms live in one focused place and borrow handling is
    /// isolated from the other diplomacy passes.
    fn advance_relation_transitions(&mut self) {
        let tick = self.tick;
        let counts = self.faction_alive_counts();
        let keys: Vec<(u8, u8)> = self.relations.keys().copied().collect();
        let mut transitions: Vec<((u8, u8), DiplomaticState, String)> = Vec::new();
        let mut trust_bumps: Vec<((u8, u8), i16)> = Vec::new();
        let mut pressure_decays: Vec<((u8, u8), u16)> = Vec::new();
        for key in keys {
            let (a, b) = key;
            let rel = self.relations[&key];
            match rel.state {
                DiplomaticState::Neutral => {
                    pressure_decays.push((key, 2));
                    if rel.pressure >= COLD_WAR_THRESHOLD {
                        transitions.push((
                            key,
                            DiplomaticState::ColdWar,
                            format!("✦ diplo ✦ F{}↔F{} tensions cool into COLD WAR", a, b),
                        ));
                    }
                }
                DiplomaticState::ColdWar => {
                    pressure_decays.push((key, 1));
                    let war_threshold = self.persona_war_threshold(a, b);
                    if rel.pressure >= war_threshold {
                        transitions.push((
                            key,
                            DiplomaticState::OpenWar,
                            format!(
                                "✦ WAR ✦ F{} declares open hostilities on F{}",
                                a, b
                            ),
                        ));
                    } else if rel.expires_tick > 0 && tick >= rel.expires_tick
                        && rel.pressure < 30
                    {
                        transitions.push((
                            key,
                            DiplomaticState::Neutral,
                            format!("✦ diplo ✦ F{}↔F{} cold war thaws", a, b),
                        ));
                    }
                }
                DiplomaticState::OpenWar => {
                    // Wars stay hot — no passive pressure decay.
                    // Check for post-war subordination: if one side
                    // is >2x the other and the weaker side is below
                    // 30% of its peak, the dominant faction vassals
                    // the loser instead of finishing them off.
                    let ca = counts.get(a as usize).copied().unwrap_or(0);
                    let cb = counts.get(b as usize).copied().unwrap_or(0);
                    let peak_a = self
                        .faction_stats
                        .get(a as usize)
                        .map(|s| s.peak_alive)
                        .unwrap_or(0);
                    let peak_b = self
                        .faction_stats
                        .get(b as usize)
                        .map(|s| s.peak_alive)
                        .unwrap_or(0);
                    let vassal_candidate = if ca >= cb.saturating_mul(2)
                        && peak_b >= 10
                        && (cb as f32) < (peak_b as f32 * 0.3)
                    {
                        Some((a, b))
                    } else if cb >= ca.saturating_mul(2)
                        && peak_a >= 10
                        && (ca as f32) < (peak_a as f32 * 0.3)
                    {
                        Some((b, a))
                    } else {
                        None
                    };
                    if let Some((overlord, vassal)) = vassal_candidate {
                        transitions.push((
                            key,
                            DiplomaticState::Vassalage { overlord },
                            format!(
                                "✦ MYTHIC ✦ F{} yields as VASSAL to F{}",
                                vassal, overlord
                            ),
                        ));
                    } else if rel.expires_tick > 0 && tick >= rel.expires_tick {
                        if rel.pressure < WAR_DE_ESCALATE_THRESHOLD {
                            transitions.push((
                                key,
                                DiplomaticState::ColdWar,
                                format!("✦ diplo ✦ F{}↔F{} truce — falls to COLD WAR", a, b),
                            ));
                        } else {
                            // Still too hot — refresh the war timer
                            // in place (no transition; just extend).
                            transitions.push((
                                key,
                                DiplomaticState::OpenWar,
                                String::new(),
                            ));
                        }
                    }
                }
                DiplomaticState::Trade => {
                    let mult = self.persona_trust_mult(a, b);
                    let gain = (2.0 * mult).round() as i16;
                    trust_bumps.push((key, gain));
                    if rel.expires_tick > 0 && tick >= rel.expires_tick {
                        if rel.trust >= NAP_TRUST_THRESHOLD {
                            transitions.push((
                                key,
                                DiplomaticState::NonAggression,
                                format!(
                                    "✦ diplo ✦ F{}↔F{} trade deepens into NON-AGGRESSION",
                                    a, b
                                ),
                            ));
                        } else {
                            transitions.push((
                                key,
                                DiplomaticState::Neutral,
                                format!("✦ diplo ✦ F{}↔F{} trade lapses", a, b),
                            ));
                        }
                    }
                }
                DiplomaticState::NonAggression => {
                    let mult = self.persona_trust_mult(a, b);
                    let gain = (1.0 * mult).round() as i16;
                    trust_bumps.push((key, gain));
                    // Betrayal check: if pressure climbs above the
                    // cold-war floor during a NAP, the peace breaks
                    // and the relation snaps to OpenWar.
                    if rel.pressure >= COLD_WAR_THRESHOLD {
                        transitions.push((
                            key,
                            DiplomaticState::OpenWar,
                            format!(
                                "✦ MYTHIC ✦ F{}↔F{} non-aggression BROKEN — war!",
                                a, b
                            ),
                        ));
                    } else if rel.expires_tick > 0 && tick >= rel.expires_tick {
                        if rel.trust >= ALLIANCE_TRUST_THRESHOLD {
                            transitions.push((
                                key,
                                DiplomaticState::Alliance,
                                format!("✦ diplo ✦ F{}↔F{} ALLIANCE forged", a, b),
                            ));
                        } else {
                            transitions.push((
                                key,
                                DiplomaticState::Neutral,
                                format!("✦ diplo ✦ F{}↔F{} NAP expires", a, b),
                            ));
                        }
                    }
                }
                DiplomaticState::Alliance => {
                    let mult = self.persona_trust_mult(a, b);
                    let gain = (1.0 * mult).round() as i16;
                    trust_bumps.push((key, gain));
                    if rel.pressure >= COLD_WAR_THRESHOLD {
                        transitions.push((
                            key,
                            DiplomaticState::OpenWar,
                            format!(
                                "✦ MYTHIC ✦ F{}↔F{} ALLIANCE BETRAYED — war!",
                                a, b
                            ),
                        ));
                    } else if rel.expires_tick > 0 && tick >= rel.expires_tick {
                        transitions.push((
                            key,
                            DiplomaticState::NonAggression,
                            format!("✦ diplo ✦ F{}↔F{} alliance winds down to NAP", a, b),
                        ));
                    }
                }
                DiplomaticState::Vassalage { overlord } => {
                    // Derive subordinate from the canonical key:
                    // if overlord is the `a` side (the smaller
                    // faction id), the vassal must be `b`, and
                    // vice versa. Correct under either ordering.
                    let subordinate = if overlord == a { b } else { a };
                    // Vassal rebellion: the vassal throws off the
                    // chain when it recovers past 70% of the
                    // overlord's size. Requires a minimum overlord
                    // size so tiny-vs-tiny pairs don't trip the
                    // rebellion on the first sample — `1 >= 0.7`
                    // would otherwise fire immediately for any
                    // 1-alive overlord.
                    let overlord_count = counts.get(overlord as usize).copied().unwrap_or(0);
                    let vassal_count = counts.get(subordinate as usize).copied().unwrap_or(0);
                    if overlord_count >= 8
                        && (vassal_count as f32) >= (overlord_count as f32 * 0.7)
                    {
                        transitions.push((
                            key,
                            DiplomaticState::ColdWar,
                            format!(
                                "✦ MYTHIC ✦ F{} throws off vassalage to F{}",
                                subordinate, overlord
                            ),
                        ));
                    }
                    // Tribute trickle — vassals feed 1 intel per
                    // sample period to their overlord. Recorded
                    // on the overlord's faction_stats intel tally.
                    if let Some(s) = self.faction_stats.get_mut(overlord as usize) {
                        s.intel = s.intel.saturating_add(1);
                    }
                }
            }
        }
        // Apply decays, trust bumps, and transitions in order.
        for (key, amt) in pressure_decays {
            if let Some(r) = self.relations.get_mut(&key) {
                r.pressure = r.pressure.saturating_sub(amt);
            }
        }
        for (key, amt) in trust_bumps {
            if let Some(r) = self.relations.get_mut(&key) {
                r.trust = r.trust.saturating_add(amt).clamp(-TRUST_CAP, TRUST_CAP);
            }
        }
        for (key, new_state, msg) in transitions {
            let fire_log = {
                let Some(r) = self.relations.get_mut(&key) else { continue };
                let expires = match new_state {
                    DiplomaticState::Trade => tick + STATE_DURATION_TRADE,
                    DiplomaticState::NonAggression => tick + STATE_DURATION_NAP,
                    DiplomaticState::Alliance => tick + STATE_DURATION_ALLIANCE,
                    DiplomaticState::ColdWar => tick + STATE_DURATION_COLD_WAR,
                    DiplomaticState::OpenWar => tick + STATE_DURATION_OPEN_WAR,
                    DiplomaticState::Neutral | DiplomaticState::Vassalage { .. } => 0,
                };
                r.expires_tick = expires;
                let rotated = r.state != new_state;
                if rotated {
                    r.state = new_state;
                    r.entered_tick = tick;
                }
                rotated && !msg.is_empty()
            };
            if fire_log {
                self.push_log(msg);
            }
        }
    }

    /// Roll an opportunistic Trade proposal between a random
    /// Neutral pair. Low base chance, boosted when either side
    /// is Opportunist. Prevents the peace track from lying
    /// dormant in quiet mid-games. Split from `advance_diplomacy`
    /// so the random-pair selection can short-circuit cleanly
    /// without dragging the rest of the pass with it — the old
    /// inline version used `else { return }` and would abort
    /// every pass downstream of itself.
    fn maybe_propose_trade(&mut self, faction_count: u8) {
        if faction_count < 2 {
            return;
        }
        if !self.rng.gen_bool(0.25) {
            return;
        }
        let tick = self.tick;
        let a = self.rng.gen_range(0..faction_count);
        let mut b = self.rng.gen_range(0..faction_count);
        if a == b {
            b = (a + 1) % faction_count;
        }
        if a == b {
            return;
        }
        let Some(key) = Self::rivalry_key(a, b) else {
            return;
        };
        let rel = self.relation(a, b);
        if !matches!(rel.state, DiplomaticState::Neutral) || rel.pressure >= 20 {
            return;
        }
        let opportunist = |p: Option<&Persona>| matches!(p, Some(Persona::Opportunist));
        let chance = if opportunist(self.personas.get(a as usize))
            || opportunist(self.personas.get(b as usize))
        {
            0.35
        } else {
            0.12
        };
        if !self.rng.gen_bool(chance) {
            return;
        }
        let entry = self.relations.entry(key).or_default();
        entry.state = DiplomaticState::Trade;
        entry.entered_tick = tick;
        entry.expires_tick = tick + STATE_DURATION_TRADE;
        self.push_log(format!("✦ diplo ✦ F{}↔F{} open TRADE channel", a, b));
    }

    /// Research accrual + tier unlock pass. Called once per faction
    /// sample period. For each alive faction, computes income from
    /// (alive count + intel delta + cures delta) plus diplomatic
    /// bonuses (Trade/Alliance pairs contribute, Vassalage skims
    /// from the vassal to the overlord), adds it to the faction's
    /// research counter, and promotes the tech tier when the next
    /// threshold is crossed. Tier transitions log a flavor line
    /// tagged with the persona so the reader can see each faction's
    /// specialization come online.
    fn advance_research(&mut self) {
        let faction_count = self.faction_stats.len();
        if faction_count == 0 {
            return;
        }
        let counts = self.faction_alive_counts();
        let diplo_mult = self.research_diplo_multipliers(faction_count);
        let mut incomes = self.compute_research_incomes(&counts, &diplo_mult);
        self.apply_vassalage_tribute_skim(&mut incomes);
        self.apply_research_incomes(&incomes);
        self.promote_tech_tiers();
    }

    /// Build the per-faction diplomatic multiplier vector for this
    /// research pass. +25% per Trade partner, +50% per Alliance,
    /// +10% per NonAggression. Partners of a given faction stack
    /// additively into the scalar that `compute_research_incomes`
    /// then applies to the faction's raw income.
    fn research_diplo_multipliers(&self, faction_count: usize) -> Vec<f32> {
        let mut diplo_mult = vec![1.0f32; faction_count];
        for (&(a, b), rel) in &self.relations {
            let bonus = match rel.state {
                DiplomaticState::Trade => 0.25,
                DiplomaticState::Alliance => 0.50,
                DiplomaticState::NonAggression => 0.10,
                _ => 0.0,
            };
            if bonus > 0.0 {
                if let Some(m) = diplo_mult.get_mut(a as usize) {
                    *m += bonus;
                }
                if let Some(m) = diplo_mult.get_mut(b as usize) {
                    *m += bonus;
                }
            }
        }
        diplo_mult
    }

    /// Compute raw research income per faction into a side buffer.
    /// Fully-dead factions (alive == 0) earn nothing but still have
    /// their `last_*_sample` refreshed so a future resurrection
    /// sees a clean delta. The side buffer lets
    /// `apply_vassalage_tribute_skim` move income between factions
    /// before it touches persistent state.
    fn compute_research_incomes(
        &mut self,
        counts: &[u32],
        diplo_mult: &[f32],
    ) -> Vec<u32> {
        let faction_count = self.faction_stats.len();
        let mut incomes = vec![0u32; faction_count];
        for (i, stats) in self.faction_stats.iter_mut().enumerate() {
            let alive = counts.get(i).copied().unwrap_or(0);
            if alive == 0 {
                stats.last_intel_sample = stats.intel;
                stats.last_cures_sample = stats.infections_cured;
                continue;
            }
            let intel_delta = stats.intel.saturating_sub(stats.last_intel_sample);
            let cures_delta = stats
                .infections_cured
                .saturating_sub(stats.last_cures_sample);
            stats.last_intel_sample = stats.intel;
            stats.last_cures_sample = stats.infections_cured;
            // Base: 1 flat + 1 per 12 alive. A 25-alive faction
            // earns ~3/sample before deltas, pushing T1 unlock to
            // roughly tick 2000 under passive play.
            let base = 1 + alive / 12;
            let earned = base + intel_delta + cures_delta;
            let mult = diplo_mult.get(i).copied().unwrap_or(1.0);
            incomes[i] = (earned as f32 * mult).round() as u32;
        }
        incomes
    }

    /// Skim 30% of each vassal's income over to its overlord.
    /// Keeps vassals productive but tilts the ledger toward the
    /// overlord to make subordination meaningful. Pure side-buffer
    /// mutation — the actual research counters aren't touched yet.
    fn apply_vassalage_tribute_skim(&self, incomes: &mut [u32]) {
        let vassal_pairs: Vec<(u8, u8)> = self
            .relations
            .iter()
            .filter_map(|(&(a, b), r)| match r.state {
                DiplomaticState::Vassalage { overlord } => {
                    let vassal = if overlord == a { b } else { a };
                    Some((overlord, vassal))
                }
                _ => None,
            })
            .collect();
        for (overlord, vassal) in vassal_pairs {
            let v_idx = vassal as usize;
            let o_idx = overlord as usize;
            if v_idx < incomes.len() && o_idx < incomes.len() {
                let skim = incomes[v_idx] * 30 / 100;
                incomes[v_idx] = incomes[v_idx].saturating_sub(skim);
                incomes[o_idx] = incomes[o_idx].saturating_add(skim);
            }
        }
    }

    /// Fold the computed incomes into each faction's persistent
    /// `research` counter. Intentionally separate from the
    /// promotion check so `push_log` in the next step can run
    /// without aliasing `faction_stats.iter_mut()`.
    fn apply_research_incomes(&mut self, incomes: &[u32]) {
        for (i, stats) in self.faction_stats.iter_mut().enumerate() {
            let income = incomes.get(i).copied().unwrap_or(0);
            stats.research = stats.research.saturating_add(income);
        }
    }

    /// Check each faction's research against the tier thresholds
    /// and promote when it crosses one. Emits a `✦ tech ✦` log
    /// line with persona-flavored effect text on every promotion.
    fn promote_tech_tiers(&mut self) {
        let mut promotions: Vec<(usize, u8)> = Vec::new();
        for (i, stats) in self.faction_stats.iter().enumerate() {
            let new_tier = if stats.research >= TECH_TIER_3_COST {
                3
            } else if stats.research >= TECH_TIER_2_COST {
                2
            } else if stats.research >= TECH_TIER_1_COST {
                1
            } else {
                0
            };
            if new_tier > stats.tech_tier {
                promotions.push((i, new_tier));
            }
        }
        for (i, new_tier) in promotions {
            let persona = self
                .personas
                .get(i)
                .copied()
                .unwrap_or(Persona::Opportunist);
            if let Some(stats) = self.faction_stats.get_mut(i) {
                stats.tech_tier = new_tier;
            }
            let effect = match (new_tier, persona) {
                (1, _) => "role specialization",
                (2, Persona::Aggressor) => "recon lenses",
                (2, Persona::Fortress) => "fortified radius",
                (2, Persona::Plague) => "virulent strains",
                (2, Persona::Opportunist) => "brokered channels",
                (3, Persona::Aggressor) => "orbital sweeps",
                (3, Persona::Fortress) => "pulse cannons",
                (3, Persona::Plague) => "endemic bloom",
                (3, Persona::Opportunist) => "wormhole brokerage",
                _ => "advancement",
            };
            self.push_log(format!(
                "✦ tech ✦ F{} {} reaches tier {} — {}",
                i,
                persona.display_name(),
                new_tier,
                effect
            ));
        }
    }

    /// Tier 3 active ability dispatcher. Called once per faction
    /// sample period after `advance_research`. Each faction at
    /// Tier 3 rolls `TECH_T3_ACTIVE_CHANCE` to fire its persona's
    /// unique ability, drawing on the existing event machinery so
    /// there's no duplicate tick logic — a Fortress just fires a
    /// free defender pulse, a Plague spawns a free worm from one
    /// of its infected hosts, etc.
    fn advance_tech_actives(&mut self) {
        let faction_count = self.faction_stats.len();
        if faction_count == 0 {
            return;
        }
        // Snapshot tier + persona up front so we can iterate
        // without aliasing self across the ability dispatch.
        let triggers: Vec<(u8, Persona)> = (0..faction_count)
            .filter_map(|i| {
                let tier = self.faction_stats.get(i)?.tech_tier;
                if tier < 3 {
                    return None;
                }
                if !self.rng.gen_bool(TECH_T3_ACTIVE_CHANCE) {
                    return None;
                }
                let persona = self.personas.get(i).copied()?;
                Some((i as u8, persona))
            })
            .collect();
        for (faction, persona) in triggers {
            match persona {
                Persona::Aggressor => self.tech_active_aggressor(faction),
                Persona::Fortress => self.tech_active_fortress(faction),
                Persona::Plague => self.tech_active_plague(faction),
                Persona::Opportunist => self.tech_active_opportunist(faction),
            }
        }
    }

    /// Aggressor T3: fire a free scanner pulse on one of the
    /// faction's alive scanners, bypassing its cooldown.
    fn tech_active_aggressor(&mut self, faction: u8) {
        let candidates: Vec<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                (n.faction == faction
                    && n.role == Role::Scanner
                    && matches!(n.state, State::Alive))
                .then_some(i)
            })
            .collect();
        if candidates.is_empty() {
            return;
        }
        let pick = candidates[self.rng.gen_range(0..candidates.len())];
        self.nodes[pick].role_cooldown = 0;
        self.nodes[pick].scan_pulse = SCANNER_PULSE_TICKS.saturating_mul(2);
        let pos = self.nodes[pick].pos;
        let (oa, ob) = octet_pair(pos);
        self.push_log(format!(
            "✦ tech ✦ F{} orbital sweep @ 10.0.{}.{}",
            faction, oa, ob
        ));
    }

    /// Fortress T3: fire a free defender pulse centered on one
    /// of the faction's alive defenders. Treated exactly like a
    /// normal defender fire by reusing the same config + node
    /// state mutation, just with the cooldown pre-cleared.
    fn tech_active_fortress(&mut self, faction: u8) {
        let candidates: Vec<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                (n.faction == faction
                    && n.role == Role::Defender
                    && matches!(n.state, State::Alive))
                .then_some(i)
            })
            .collect();
        if candidates.is_empty() {
            return;
        }
        let pick = candidates[self.rng.gen_range(0..candidates.len())];
        self.nodes[pick].role_cooldown = 0;
        self.nodes[pick].pulse = 4;
        // Queue a patch wave from the defender's position — the
        // existing patch_waves pipeline will sweep it outward and
        // clear nearby infections on subsequent ticks.
        let origin = self.nodes[pick].pos;
        self.patch_waves.push(PatchWave {
            origin,
            radius: 0,
        });
        let (oa, ob) = octet_pair(origin);
        self.push_log(format!(
            "✦ tech ✦ F{} pulse cannon @ 10.0.{}.{}",
            faction, oa, ob
        ));
    }

    /// Plague T3: force a fresh outbreak on two distinct random
    /// alive nodes anywhere on the mesh, bypassing immunity
    /// windows and the normal seed-rate cap. Plants the faction's
    /// bias as a mid-run pressure release valve. Uses shuffle +
    /// take so the two picks are guaranteed distinct — the
    /// previous version drew two independent indices with
    /// replacement, which could double-select the same node and
    /// silently seed only one infection.
    fn tech_active_plague(&mut self, faction: u8) {
        let mut alive_targets: Vec<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                (matches!(n.state, State::Alive)
                    && n.infection.is_none()
                    && !n.role.is_virus_immune()
                    && !self.c2_nodes.contains(&i))
                .then_some(i)
            })
            .collect();
        if alive_targets.is_empty() {
            return;
        }
        let cure_resist = self.cfg.virus_cure_resist;
        let strain = self.rng.gen_range(0..STRAIN_COUNT as u8);
        alive_targets.shuffle(&mut self.rng);
        let picks: Vec<NodeId> = alive_targets.into_iter().take(2).collect();
        let mut seeded = 0u32;
        for id in picks {
            if self.nodes[id].infection.is_none() {
                self.nodes[id].infection = Some(Infection::seeded(strain, cure_resist));
                seeded += 1;
            }
        }
        if seeded > 0 {
            let name = self.strain_name(strain);
            self.push_log(format!(
                "✦ tech ✦ F{} endemic bloom — {} hosts seeded with {}",
                faction, seeded, name
            ));
        }
    }

    /// Opportunist T3: spawn a free wormhole somewhere on the mesh.
    /// Uses the same two-random-alive-nodes pick that the periodic
    /// `maybe_wormhole` event uses, just wrapped in a tech-tag
    /// logline so the reader sees which faction triggered it.
    fn tech_active_opportunist(&mut self, faction: u8) {
        let alive: Vec<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                (matches!(n.state, State::Alive) && !self.c2_nodes.contains(&i)).then_some(i)
            })
            .collect();
        if alive.len() < 2 {
            return;
        }
        let a = alive[self.rng.gen_range(0..alive.len())];
        let mut b = alive[self.rng.gen_range(0..alive.len())];
        while b == a {
            b = alive[self.rng.gen_range(0..alive.len())];
        }
        let life = self.cfg.wormhole_life_ticks;
        self.wormholes.push(Wormhole {
            a: self.nodes[a].pos,
            b: self.nodes[b].pos,
            age: 0,
            life,
        });
        let (oa, ob) = octet_pair(self.nodes[a].pos);
        self.push_log(format!(
            "✦ tech ✦ F{} brokered wormhole @ 10.0.{}.{}",
            faction, oa, ob
        ));
    }

    /// Resolve the per-faction tech bonuses for this faction into
    /// a single `TechEffects` struct. Call sites read one scalar
    /// from the returned struct (e.g. `self.tech_effects(f).role_intensity`)
    /// instead of calling five separate helper methods. Unknown
    /// factions (no stats entry) return `TechEffects::default()`
    /// — all fields resolve to their no-op values.
    ///
    /// Effect table:
    /// - T1+ (all personas): `role_intensity` rises above 1.0.
    ///   Deliberately plateaus at T2 — T3's reward is the active
    ///   ability in `advance_tech_actives`, not stronger role bias.
    /// - T2+ Fortress: `defender_radius_bonus = 2`.
    /// - T2+ Aggressor: `scanner_period_mult = 0.65`.
    /// - T2+ Plague: `worm_spawn_mult = 2.0`.
    /// - T2+ Opportunist: `bridge_mult = 2.0`.
    pub fn tech_effects(&self, faction: u8) -> TechEffects {
        let Some(stats) = self.faction_stats.get(faction as usize) else {
            return TechEffects::default();
        };
        let tier = stats.tech_tier;
        let persona = self
            .personas
            .get(faction as usize)
            .copied()
            .unwrap_or(Persona::Opportunist);
        let role_intensity = match tier {
            0 => 1.0,
            1 => 1.35,
            _ => 1.6,
        };
        let (defender_radius_bonus, scanner_period_mult, worm_spawn_mult, bridge_mult) =
            if tier >= 2 {
                match persona {
                    Persona::Fortress => (2, 1.0, 1.0, 1.0),
                    Persona::Aggressor => (0, 0.65, 1.0, 1.0),
                    Persona::Plague => (0, 1.0, 2.0, 1.0),
                    Persona::Opportunist => (0, 1.0, 1.0, 2.0),
                }
            } else {
                (0, 1.0, 1.0, 1.0)
            };
        TechEffects {
            role_intensity,
            defender_radius_bonus,
            scanner_period_mult,
            worm_spawn_mult,
            bridge_mult,
        }
    }

    /// Recompute which faction (if any) currently holds dominance
    /// and emit a log line when the holder changes. Dominance is
    /// purely a tracked stat — the sim never auto-ends on it, the
    /// holder is just surfaced in the UI and summary screen.
    fn maybe_declare_victory(&mut self) {
        if self.faction_stats.len() < 2 {
            return;
        }
        // Count alive per faction.
        let mut counts = vec![0usize; self.faction_stats.len()];
        let mut total_alive: usize = 0;
        for n in &self.nodes {
            if matches!(n.state, State::Alive) {
                total_alive += 1;
                if let Some(slot) = counts.get_mut(n.faction as usize) {
                    *slot += 1;
                }
            }
        }
        if total_alive < 20 {
            return;
        }
        // Last-C2-standing check first.
        let alive_c2s: Vec<u8> = self
            .c2_nodes
            .iter()
            .filter_map(|&id| {
                if matches!(self.nodes[id].state, State::Alive) {
                    Some(self.nodes[id].faction)
                } else {
                    None
                }
            })
            .collect();
        let new_dominant: Option<u8> = if alive_c2s.len() == 1 {
            Some(alive_c2s[0])
        } else {
            // Alive-majority: one faction holds >= VICTORY_ALIVE_FRACTION.
            let threshold = (total_alive as f32 * VICTORY_ALIVE_FRACTION) as usize;
            counts
                .iter()
                .enumerate()
                .max_by_key(|(_, &c)| c)
                .filter(|&(_, &c)| c >= threshold)
                .map(|(i, _)| i as u8)
        };
        if new_dominant != self.current_dominant {
            if let Some(prev) = self.current_dominant {
                if new_dominant.is_none() {
                    self.push_log(format!(
                        "F{} loses dominance — the mesh fragments",
                        prev
                    ));
                }
            }
            if let Some(winner) = new_dominant {
                let pct = counts
                    .get(winner as usize)
                    .copied()
                    .map(|c| (c as f32 / total_alive as f32) * 100.0)
                    .unwrap_or(0.0);
                self.dominance_shifts = self.dominance_shifts.saturating_add(1);
                if alive_c2s.len() == 1 {
                    self.push_log(format!(
                        "✦ DOMINANCE ✦ F{} is the last C2 standing",
                        winner
                    ));
                } else {
                    self.push_log(format!(
                        "✦ DOMINANCE ✦ F{} controls {:.0}% of the mesh",
                        winner, pct
                    ));
                }
            }
            self.current_dominant = new_dominant;
        }
    }

    /// Probabilistically wake any active sleeper agents. A waking
    /// sleeper flips its visible faction to its hidden true faction,
    /// gets a mutated_flash, and seeds an infection on its host
    /// node so the betrayal lands with weight. Logs the reveal.
    fn maybe_wake_sleepers(&mut self) {
        if self.cfg.sleeper_wake_chance <= 0.0 {
            return;
        }
        let mut to_wake: Vec<NodeId> = Vec::new();
        for (id, n) in self.nodes.iter().enumerate() {
            if n.sleeper_true_faction.is_none() {
                continue;
            }
            if !matches!(n.state, State::Alive) || n.dying_in > 0 {
                continue;
            }
            if self.rng.gen_bool(self.cfg.sleeper_wake_chance as f64) {
                to_wake.push(id);
            }
        }
        let cure_resist = self.cfg.virus_cure_resist;
        for id in to_wake {
            let Some(true_f) = self.nodes[id].sleeper_true_faction else {
                continue;
            };
            let old_faction = self.nodes[id].faction;
            let pos = self.nodes[id].pos;
            self.nodes[id].faction = true_f;
            self.nodes[id].sleeper_true_faction = None;
            self.nodes[id].mutated_flash = 12;
            // Plant a fresh strain on the host as the act of
            // sabotage so the betrayal has a visible mechanical
            // effect, not just a faction recolor.
            if self.nodes[id].infection.is_none() {
                let strain = self.rng.gen_range(0..STRAIN_COUNT as u8);
                self.nodes[id].infection = Some(Infection::seeded(strain, cure_resist));
            }
            // The reveal feeds the rivalry between the host's old
            // faction and its true faction.
            self.bump_rivalry(old_faction, true_f, 12);
            let (a, b) = octet_pair(pos);
            self.push_log(format!(
                "✦ sleeper ✦ F{} mole revealed in F{} @ 10.0.{}.{}",
                true_f, old_faction, a, b
            ));
        }
    }

    /// Scan for nodes that have earned legendary status and assign
    /// them a stable name from LEGENDARY_NAME_POOL. The promotion
    /// rule is "alive + long-lived + reproductively successful":
    /// age past LEGENDARY_MIN_AGE, children_spawned past
    /// LEGENDARY_MIN_CHILDREN, not a C2 (C2s are faction-level, not
    /// characters), not already legendary.
    fn maybe_promote_legendary(&mut self) {
        let mut promoted: Vec<(NodeId, (i16, i16), u16)> = Vec::new();
        let now = self.tick;
        for (id, n) in self.nodes.iter().enumerate() {
            if n.legendary_name != u16::MAX {
                continue;
            }
            if !matches!(n.state, State::Alive) || n.dying_in > 0 {
                continue;
            }
            if self.is_c2(id) {
                continue;
            }
            if now.saturating_sub(n.born) < LEGENDARY_MIN_AGE {
                continue;
            }
            if n.children_spawned < LEGENDARY_MIN_CHILDREN {
                continue;
            }
            // Hash the node id into the name pool so the same seed
            // always picks the same names deterministically.
            let pool_len = LEGENDARY_NAME_POOL.len() as u16;
            let idx = ((id as u32).wrapping_mul(2654435761) as u16) % pool_len;
            promoted.push((id, n.pos, idx));
        }
        for (id, pos, idx) in promoted {
            self.nodes[id].legendary_name = idx;
            self.nodes[id].mutated_flash = 10;
            let name = LEGENDARY_NAME_POOL[idx as usize];
            let (a, b) = octet_pair(pos);
            self.push_log(format!("✦ legend ✦ {} rises @ 10.0.{}.{}", name, a, b));
        }
    }

    /// Engage faction favoritism for `faction` for
    /// `FAVOR_DURATION_TICKS` ticks. Used by the 1-9 hotkeys.
    /// Refuses silently when the faction id is out of range so
    /// stray key presses don't surprise the user.
    pub fn favor_faction(&mut self, faction: u8) {
        if (faction as usize) >= self.faction_stats.len() {
            return;
        }
        self.favored_faction = Some(faction);
        self.favor_expires_tick = self.tick + FAVOR_DURATION_TICKS;
        self.push_log(format!(
            "✦ favored ✦ F{} gets a spawn boost ({}t)",
            faction, FAVOR_DURATION_TICKS
        ));
    }

    /// True if the given faction currently holds an active
    /// favoritism boost.
    pub fn is_favored(&self, faction: u8) -> bool {
        self.favored_faction == Some(faction) && self.tick < self.favor_expires_tick
    }

    /// Drop a patch wave at `origin`. Uses the same geometry as
    /// the heartbeat-driven waves so the visual/mechanic is
    /// identical; it's just triggered by a keybind instead of the
    /// timer. Used by the cursor-action hotkey 'p'.
    pub fn inject_patch_wave(&mut self, origin: (i16, i16)) {
        self.patch_waves.push(PatchWave { origin, radius: 0 });
        // Flash any node sitting at the cursor so the inject has
        // a visible mesh-side response beyond the log line.
        if let Some(n) = self.nodes.iter_mut().find(|n| n.pos == origin) {
            n.mutated_flash = 8;
            n.pulse = 4;
        }
        let (a, b) = octet_pair(origin);
        self.push_log(format!("patch wave injected @ 10.0.{}.{}", a, b));
    }

    /// Force the alive node (if any) nearest `origin` to fire a
    /// scanner ping. If no alive node sits on the exact cell, the
    /// closest Chebyshev neighbor within radius 2 is used. Used by
    /// the cursor-action hotkey 's'.
    pub fn inject_scanner_pulse(&mut self, origin: (i16, i16)) {
        let pick = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| matches!(n.state, State::Alive))
            .min_by_key(|(_, n)| {
                (n.pos.0 - origin.0).abs().max((n.pos.1 - origin.1).abs())
            });
        let Some((id, node)) = pick else {
            self.push_log("scanner pulse refused: no alive node".to_string());
            return;
        };
        let dist = (node.pos.0 - origin.0).abs().max((node.pos.1 - origin.1).abs());
        if dist > 4 {
            self.push_log("scanner pulse refused: no nearby node".to_string());
            return;
        }
        self.nodes[id].scan_pulse = SCANNER_PULSE_TICKS.saturating_mul(2);
        self.nodes[id].role_cooldown = 0;
        self.nodes[id].mutated_flash = 8;
        let pos = self.nodes[id].pos;
        self.log_node(pos, "scanner pulse injected");
    }

    /// Plant a fresh C2 / new faction at `origin` if the cell is
    /// empty and in-bounds. The new faction gets its own persona,
    /// random palette slot, and full HP reservoir. Used by the
    /// cursor-action hotkey 'c'.
    pub fn inject_c2(&mut self, origin: (i16, i16)) {
        if origin.0 < 0
            || origin.1 < 0
            || origin.0 >= self.bounds.0
            || origin.1 >= self.bounds.1
        {
            self.push_log("c2 plant refused: out of bounds".to_string());
            return;
        }
        if self.occupied.contains(&origin) {
            self.push_log("c2 plant refused: cell occupied".to_string());
            return;
        }
        let new_faction = self.c2_nodes.len() as u8;
        let mut node = Node::fresh(origin, None, self.tick, Role::Relay, 0);
        node.faction = new_faction;
        node.pwn_resist = C2_INITIAL_HP;
        node.mutated_flash = 12;
        let id = self.nodes.len();
        self.nodes.push(node);
        self.occupied.insert(origin);
        self.c2_nodes.push(id);
        self.faction_stats.push(FactionStats::default());
        let persona = match self.rng.gen_range(0..4u8) {
            0 => Persona::Aggressor,
            1 => Persona::Fortress,
            2 => Persona::Plague,
            _ => Persona::Opportunist,
        };
        self.personas.push(persona);
        let palette_len = crate::theme::theme().faction_palette.len().max(1);
        self.faction_colors.push(self.rng.gen_range(0..palette_len));
        let (a, b) = octet_pair(origin);
        self.push_log(format!(
            "✦ c2 planted ✦ F{} online @ 10.0.{}.{}",
            new_faction, a, b
        ));
    }

    /// Spawn a wormhole connecting `origin` to a random alive cell
    /// elsewhere on the mesh. Used by the cursor-action hotkey 'w'.
    pub fn inject_wormhole(&mut self, origin: (i16, i16)) {
        let alive: Vec<(i16, i16)> = self
            .nodes
            .iter()
            .filter(|n| matches!(n.state, State::Alive) && n.pos != origin)
            .map(|n| n.pos)
            .collect();
        if alive.is_empty() {
            self.push_log("wormhole refused: no other alive node".to_string());
            return;
        }
        let other = alive[self.rng.gen_range(0..alive.len())];
        let (oa, ob) = octet_pair(origin);
        let (ta, tb) = octet_pair(other);
        self.push_log(format!(
            "wormhole injected 10.0.{}.{} ↔ 10.0.{}.{}",
            oa, ob, ta, tb
        ));
        let life = self.cfg.wormhole_life_ticks;
        self.wormholes.push(Wormhole {
            a: origin,
            b: other,
            age: 0,
            life,
        });
    }

    /// Canonical-pair key for the rivalry map. Always (min, max) so
    /// either argument order produces the same lookup. Returns None
    /// if both factions are the same — self-rivalries are nonsense.
    fn rivalry_key(a: u8, b: u8) -> Option<(u8, u8)> {
        if a == b {
            None
        } else {
            Some((a.min(b), a.max(b)))
        }
    }

    /// Read the current hostile pressure between two factions.
    /// Zero if they've never interacted.
    pub fn rivalry_pressure(&self, a: u8, b: u8) -> u16 {
        Self::rivalry_key(a, b)
            .and_then(|k| self.relations.get(&k).map(|r| r.pressure))
            .unwrap_or(0)
    }

    /// Look up the full `Relation` record for a pair. Returns a
    /// fresh default Neutral relation if the pair has never
    /// interacted — callers can inspect `.state` / `.trust` /
    /// `.pressure` uniformly without worrying about absence.
    pub fn relation(&self, a: u8, b: u8) -> Relation {
        Self::rivalry_key(a, b)
            .and_then(|k| self.relations.get(&k).copied())
            .unwrap_or_default()
    }

    /// Convenience wrapper — just the `DiplomaticState` of a pair.
    pub fn relation_state(&self, a: u8, b: u8) -> DiplomaticState {
        self.relation(a, b).state
    }

    /// Bump hostile pressure on a pair by `amount`, clamped to
    /// `RIVALRY_CAP`. No-op for self-pairs. Kept under the old
    /// `bump_rivalry` name so the existing skirmish / worm-cross
    /// / siege / sleeper callers don't need to know the relation
    /// map exists — they just report hostilities and this records
    /// it. The state machine picks up the pressure on the next
    /// `advance_diplomacy` pass.
    pub fn bump_rivalry(&mut self, a: u8, b: u8, amount: u16) {
        if let Some(key) = Self::rivalry_key(a, b) {
            let tick = self.tick;
            let entry = self.relations.entry(key).or_insert_with(|| Relation {
                entered_tick: tick,
                ..Relation::default()
            });
            entry.pressure = entry.pressure.saturating_add(amount).min(RIVALRY_CAP);
            // Betrayal: taking hostile action in a peaceful state
            // shaves trust proportionally, making broken peace
            // visible as a step change rather than a slow drift.
            if matches!(
                entry.state,
                DiplomaticState::Trade
                    | DiplomaticState::NonAggression
                    | DiplomaticState::Alliance
            ) {
                entry.trust = entry
                    .trust
                    .saturating_sub(amount as i16)
                    .clamp(-TRUST_CAP, TRUST_CAP);
            }
        }
    }

    /// Bump cooperative trust on a pair by `amount` (signed),
    /// clamped to ±`TRUST_CAP`. Called by the state machine
    /// during peaceful dwell and by any future "positive event"
    /// hooks. No-op for self-pairs.
    #[allow(dead_code)]
    pub fn bump_trust(&mut self, a: u8, b: u8, amount: i16) {
        if let Some(key) = Self::rivalry_key(a, b) {
            let tick = self.tick;
            let entry = self.relations.entry(key).or_insert_with(|| Relation {
                entered_tick: tick,
                ..Relation::default()
            });
            entry.trust = entry
                .trust
                .saturating_add(amount)
                .clamp(-TRUST_CAP, TRUST_CAP);
        }
    }

    /// If `vassal` is subordinated under an overlord in the current
    /// diplomacy map, return the overlord's faction id. Used by
    /// tribute collection in `advance_diplomacy` and by the UI to
    /// label vassal relations.
    #[allow(dead_code)]
    pub fn vassal_of(&self, vassal: u8) -> Option<u8> {
        for (&(a, b), r) in &self.relations {
            if let DiplomaticState::Vassalage { overlord } = r.state {
                let subordinate = if overlord == a { b } else { a };
                if subordinate == vassal {
                    return Some(overlord);
                }
            }
        }
        None
    }

    /// Palette slot for a given faction. Used by the renderer to
    /// pick the faction hue via the shuffled `faction_colors` table
    /// so each run produces distinct color-to-faction mappings.
    /// Falls back to the faction id itself if the table is short.
    pub fn faction_color_index(&self, faction: u8) -> usize {
        self.faction_colors
            .get(faction as usize)
            .copied()
            .unwrap_or(faction as usize)
    }

    /// True if any alive node within Chebyshev distance 1 of `pos`
    /// has the given role. Used by the role-synergy bonuses (Tower
    /// near Defender, Scanner near Beacon, Exfil near Router) so
    /// adjacent role combos reward tactical spawn placement.
    pub(crate) fn has_neighbor_role(&self, pos: (i16, i16), role: Role) -> bool {
        for dx in -1i16..=1 {
            for dy in -1i16..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let np = (pos.0 + dx, pos.1 + dy);
                for n in &self.nodes {
                    if n.pos == np
                        && n.role == role
                        && matches!(n.state, State::Alive)
                        && n.dying_in == 0
                    {
                        return true;
                    }
                }
            }
        }
        false
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

    /// Unified periodic-event gate. Returns true once every `period`
    /// ticks (skipping tick 0) AND only when a `chance` roll fires.
    /// Most `maybe_*` event handlers collapse to a single call.
    /// Pass `chance = 1.0` for period-only firing.
    fn roll_periodic(&mut self, period: u64, chance: f32) -> bool {
        if period == 0 || self.tick == 0 || !self.tick.is_multiple_of(period) {
            return false;
        }
        if chance >= 1.0 {
            return true;
        }
        if chance <= 0.0 {
            return false;
        }
        self.rng.gen_bool(chance as f64)
    }

    /// True if factions `a` and `b` currently have any peaceful
    /// or subordinated relationship that should block aggression
    /// between them — Alliance, NonAggression, Trade, or a
    /// Vassalage pair. Kept under the old `allied` name because
    /// every caller (skirmish target filter, worm crossing block,
    /// bridge suppression) is asking the same question: "is this
    /// pair off-limits for hostile actions right now?"
    pub fn allied(&self, a: u8, b: u8) -> bool {
        if a == b {
            return true;
        }
        matches!(
            self.relation_state(a, b),
            DiplomaticState::Alliance
                | DiplomaticState::NonAggression
                | DiplomaticState::Trade
                | DiplomaticState::Vassalage { .. }
        )
    }

    /// Post-cure immunity duration in ticks under the current era's
    /// `immunity_mult`. Call sites that previously assigned
    /// `IMMUNITY_DURATION_TICKS` directly now assign this value so the
    /// "Winter of Quarantine" era can stretch immunity 5×.
    pub fn era_immunity_ticks(&self) -> u16 {
        let base = IMMUNITY_DURATION_TICKS as f32;
        let scaled = base * self.era_rules.immunity_mult;
        scaled.clamp(1.0, u16::MAX as f32) as u16
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

/// Number of samples kept in each faction's alive-count history.
pub const FACTION_HISTORY_LEN: usize = 8;
/// Number of samples kept in the global activity history window.
/// Larger than per-faction because the activity panel is a wider
/// braille graph.
pub const ACTIVITY_HISTORY_LEN: usize = 64;
/// Tick interval between FactionStats.history samples.
const FACTION_SAMPLE_PERIOD: u64 = 50;

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
            // C2s ship with a big pwn_resist reservoir that enemy
            // worm strikes drain; at zero the C2 collapses and its
            // whole subtree cascades. This is the primary path to
            // seeing a C2 actually fall during a run.
            node.pwn_resist = C2_INITIAL_HP;
            let id = nodes.len();
            nodes.push(node);
            occupied.insert(pos);
            c2_nodes.push(id);
            logs.push_back((format!("c2[{}] online @ {},{}", i, pos.0, pos.1), 1));
        }

        // Roll 2-4 fixed hotspot fiber zones at world creation.
        // Each hotspot is a rectangle in the middle of the mesh
        // (leaving a margin on all sides) with sides in [5..9].
        // They're placed without overlap checks — a little overlap
        // produces double-dense "super zones" which is fine
        // flavor.
        let hotspot_count = rng.gen_range(2..=4);
        let mut hotspots_init: Vec<Hotspot> = Vec::with_capacity(hotspot_count);
        for _ in 0..hotspot_count {
            if bounds.0 < 12 || bounds.1 < 12 {
                break;
            }
            let w_side = rng.gen_range(5..=9).min(bounds.0 - 6);
            let h_side = rng.gen_range(5..=9).min(bounds.1 - 6);
            let x0 = rng.gen_range(3..(bounds.0 - w_side - 3));
            let y0 = rng.gen_range(3..(bounds.1 - h_side - 3));
            hotspots_init.push(Hotspot {
                min: (x0, y0),
                max: (x0 + w_side, y0 + h_side),
            });
        }
        for (i, h) in hotspots_init.iter().enumerate() {
            logs.push_back((
                format!(
                    "fiber zone #{} {},{}..{},{}",
                    i, h.min.0, h.min.1, h.max.0, h.max.1
                ),
                1,
            ));
        }

        // Pick a persona per faction before moving rng into self.
        let personas: Vec<Persona> = (0..count)
            .map(|_| match rng.gen_range(0..4) {
                0 => Persona::Aggressor,
                1 => Persona::Fortress,
                2 => Persona::Plague,
                _ => Persona::Opportunist,
            })
            .collect();
        for (i, p) in personas.iter().enumerate() {
            logs.push_back((
                format!("c2[{}] persona = {}", i, p.display_name()),
                1,
            ));
        }
        // Shuffle the theme faction palette so each run starts with
        // a different color-to-faction mapping. The palette length
        // is read through the theme singleton; we only pick indices
        // here so the world layer stays independent of Color.
        let palette_len = crate::theme::theme().faction_palette.len().max(1);
        let mut faction_colors: Vec<usize> = (0..palette_len).collect();
        faction_colors.shuffle(&mut rng);
        // If there are more factions than palette slots, wrap with a
        // secondary offset so consecutive wraparounds don't repeat.
        while faction_colors.len() < count {
            faction_colors.push(rng.gen_range(0..palette_len));
        }
        faction_colors.truncate(count.max(palette_len));

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
            ddos_waves: Vec::new(),
            wormholes: Vec::new(),
            next_branch_id: 1,
            storm_until: 0,
            storm_since: 0,
            storm_dir: (0, 1),
            strain_names,
            faction_stats: vec![FactionStats::default(); count],
            mythic_pandemic_seen: false,
            activity_history: VecDeque::with_capacity(ACTIVITY_HISTORY_LEN),
            relations: HashMap::new(),
            outages: Vec::new(),
            partitions: Vec::new(),
            personas,
            faction_colors,
            current_dominant: None,
            dominance_shifts: 0,
            hotspots: hotspots_init,
            favored_faction: None,
            favor_expires_tick: 0,
            strain_patents: vec![None; STRAIN_COUNT],
            era_rules: era_rules_for(0).0,
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
        // a new named era and swaps in its rule set. Every live
        // multiplier (spawn, loss, cascade, virus spread, mutation,
        // immunity, assimilation cadence, bridge chance, exfil period)
        // rebinds to the new era on this tick. Epoch periods shorter
        // than 2 would log a transition every tick and thrash the
        // scrollback, so anything under that floor is treated as
        // disabled.
        let epoch_period = self.cfg.epoch_period;
        if epoch_period >= 2 && self.tick > 0 && self.tick.is_multiple_of(epoch_period) {
            let idx = (self.tick / epoch_period) as usize;
            let (rules, summary) = era_rules_for(idx);
            self.era_rules = rules;
            let name = ERA_NAMES[idx % ERA_NAMES.len()];
            if summary.is_empty() {
                self.push_log(format!("✦ era ✦ {}: {}", idx, name));
            } else {
                self.push_log(format!("✦ era ✦ {}: {} — {}", idx, name, summary));
            }
        }

        // Network storm: rare chaotic burst that spikes spawn + loss for
        // a short window. Logged at start and end so the phase reads
        // clearly in the log.
        self.maybe_storm();
        self.maybe_ddos();
        self.advance_ddos_waves();
        self.maybe_wormhole();
        self.advance_wormholes();
        self.maybe_isp_outage();
        self.advance_outages();
        self.maybe_partition();
        self.advance_partitions();
        if self.cfg.sleeper_wake_period > 0
            && self.tick.is_multiple_of(self.cfg.sleeper_wake_period)
        {
            self.maybe_wake_sleepers();
        }
        self.maybe_assimilate();
        self.maybe_border_skirmish();

        // Sample faction alive counts for the header sparkline.
        if self.tick.is_multiple_of(FACTION_SAMPLE_PERIOD) {
            self.sample_faction_history();
            // Reactive persona shifts based on current vs peak/avg.
            self.maybe_shift_personas();
            // Check for legendary-node promotions on the same cadence.
            self.maybe_promote_legendary();
            // Diplomacy state machine: decays pressure, accumulates
            // trust, fires transitions between Neutral ↔ ColdWar ↔
            // OpenWar and Neutral → Trade → NonAggression →
            // Alliance, handles Vassalage subordination, and rolls
            // Trade proposals between quiet Neutral pairs.
            self.advance_diplomacy();
            // Research accrual + tier unlocks, then Tier 3 active
            // ability rolls for any faction that just crossed (or
            // already sits at) the top tier. Runs after diplomacy
            // so Trade/Alliance bonuses land on the current tick.
            self.advance_research();
            self.advance_tech_actives();
            // Check for a dominance victory condition.
            self.maybe_declare_victory();
            // Let any critically-weakened faction trigger its
            // scorched-earth protocol as a last defiance.
            self.maybe_scorched_earth();
            // Collect strain-patent royalties from rival hosts.
            self.collect_strain_patents();
            // Wake any sleeper-lattice edges whose triggers fired.
            self.activate_sleeper_lattice();
        }
        // Expire the faction favoritism boost.
        if self.favored_faction.is_some() && self.tick >= self.favor_expires_tick {
            self.favored_faction = None;
        }

        // Phase 1: growth — add new nodes and extend link animations.
        self.try_spawn();
        self.advance_links();

        // Phase 2: traveler motion — anything moving along existing links.
        self.decay_link_load();
        self.advance_packets();
        self.advance_link_overloads();
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
        self.fire_hunter_culls();

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
            // Synergy: a Tower adjacent to a Defender regenerates one
            // pwn_resist charge per heartbeat, capped at twice the
            // configured tower spawn pool. Encourages clustered
            // fortifications around defender lattices.
            let tower_cap = self.cfg.tower_pwn_resist.saturating_mul(2).max(4);
            let tower_ids: Vec<NodeId> = self
                .nodes
                .iter()
                .enumerate()
                .filter_map(|(i, n)| {
                    if matches!(n.state, State::Alive)
                        && n.role == Role::Tower
                        && n.pwn_resist < tower_cap
                    {
                        Some(i)
                    } else {
                        None
                    }
                })
                .collect();
            for id in tower_ids {
                let pos = self.nodes[id].pos;
                if self.has_neighbor_role(pos, Role::Defender) {
                    self.nodes[id].pwn_resist =
                        self.nodes[id].pwn_resist.saturating_add(1);
                    self.nodes[id].shield_flash = 4;
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

    /// Decay one step of traffic load, breach TTL, and burn/quarantine
    /// state from every link. Called at the top of the motion phase so
    /// the add/decay pair stays symmetric. Decay is load-proportional
    /// (`max(1, load/4)`) so hot links cool aggressively — short bursts
    /// snap back instead of lingering at the ceiling.
    fn decay_link_load(&mut self) {
        for link in self.links.iter_mut() {
            let step = (link.load / 4).max(1);
            link.load = link.load.saturating_sub(step);
            link.breach_ttl = link.breach_ttl.saturating_sub(1);
            link.quarantined = link.quarantined.saturating_sub(1);
            // burn_ticks climbs while hot, unwinds while cool.
            if link.load >= HOT_LINK {
                link.burn_ticks = link.burn_ticks.saturating_add(1);
            } else if link.burn_ticks > 0 {
                link.burn_ticks -= 1;
            }
        }
    }

    /// React to sustained congestion: upgrade child endpoints into
    /// Routers when a link has been hot for a while, and collapse
    /// links that stay hot past the upper threshold. Called right
    /// after `advance_packets` so the decisions are based on the
    /// load snapshot the packets just observed.
    fn advance_link_overloads(&mut self) {
        // Pass 1: collect candidates without borrowing self mutably.
        let mut upgrade_candidates: Vec<NodeId> = Vec::new();
        let mut collapse_ids: Vec<usize> = Vec::new();
        for (li, link) in self.links.iter().enumerate() {
            if link.quarantined > 0 {
                continue;
            }
            if link.burn_ticks >= LINK_COLLAPSE_THRESHOLD {
                collapse_ids.push(li);
                continue;
            }
            if link.burn_ticks == ROUTER_UPGRADE_THRESHOLD
                && link.kind == LinkKind::Parent
            {
                upgrade_candidates.push(link.b);
            }
        }

        // Pass 2: router upgrades. Bypasses `is_mutation_locked` on
        // purpose — this is the mesh adapting to pressure in place.
        for id in upgrade_candidates {
            if self.is_c2(id) {
                continue;
            }
            let node = &self.nodes[id];
            if node.role == Role::Router
                || !matches!(node.state, State::Alive)
                || node.dying_in > 0
            {
                continue;
            }
            // Still respect honeypot stealth and defender immunity.
            if matches!(node.role, Role::Honeypot | Role::Defender) {
                continue;
            }
            if self.rng.gen_bool(ROUTER_UPGRADE_CHANCE) {
                let pos = node.pos;
                self.nodes[id].role = Role::Router;
                self.nodes[id].mutated_flash = 8;
                self.log_node(pos, "upgraded → router");
            }
        }

        // Pass 3: link collapses. Flush traffic, quarantine the link,
        // stun both endpoints, and emit a shockwave at the midpoint.
        for li in collapse_ids {
            let (mid, a, b) = {
                let link = &self.links[li];
                let mid = link
                    .path
                    .get(link.path.len() / 2)
                    .copied()
                    .unwrap_or((0, 0));
                (mid, link.a, link.b)
            };
            self.packets.retain(|p| p.link_id != li);
            self.worms.retain(|w| w.link_id != li);
            let link = &mut self.links[li];
            link.load = 0;
            link.burn_ticks = 0;
            link.quarantined = LINK_QUARANTINE_TICKS;
            // Stun endpoints. Cap via the DDoS ceiling so overlapping
            // collapses can't disable a node forever.
            const OVERLOAD_STUN: u16 = 120;
            const OVERLOAD_CAP: u16 = 500;
            for endpoint in [a, b] {
                let n = &mut self.nodes[endpoint];
                n.role_cooldown = n.role_cooldown.saturating_add(OVERLOAD_STUN).min(OVERLOAD_CAP);
                n.scan_pulse = n.scan_pulse.max(6);
            }
            self.emit_cascade_effects(mid, 8);
            self.push_log("⚡ LINK OVERLOAD — router core melted".to_string());
        }
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

// Unit tests live in the sibling file src/world/tests.rs, picked
// up automatically by Rust's module resolution.
#[cfg(test)]
mod tests;

