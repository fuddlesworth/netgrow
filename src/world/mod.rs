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
pub const GHOST_ECHO_TICKS: u8 = 60;

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

/// Minimum age (in ticks) a node needs before it can be promoted to
/// legendary status. Combined with `LEGENDARY_MIN_CHILDREN` to gate
/// the rare long-lived, reproductive characters.
pub const LEGENDARY_MIN_AGE: u64 = 1200;
/// Minimum number of direct children a node must have spawned to
/// qualify for a legendary name.
pub const LEGENDARY_MIN_CHILDREN: u16 = 8;

/// Maximum value any rivalry pair can hold. Caps the multiplier so
/// even ancient feuds eventually plateau instead of melting events.
pub const RIVALRY_CAP: u16 = 200;

/// Rivalry pressure threshold that promotes a feud to an open war
/// declaration. Crossing it emits a mythic log line and applies a
/// flat 3x skirmish chance multiplier on top of the rivalry amp.
pub const WAR_DECLARATION_THRESHOLD: u16 = 100;
/// Duration in ticks that an open war stays active after being
/// declared. Keeps the amplification window bounded so wars don't
/// consume the whole run.
pub const WAR_DURATION_TICKS: u64 = 500;

/// Fraction of total alive nodes a single faction needs to hold
/// to be counted as the currently-dominant faction. Fires a log
/// line on transitions, never ends the run — the sim keeps
/// going until the user quits.
pub const VICTORY_ALIVE_FRACTION: f32 = 0.60;

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
    pub alliances: Vec<Alliance>,
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
    /// Per-faction-pair "war pressure". Indexed by canonical pair
    /// `(min(a,b), max(a,b))`. Accumulates from cross-faction kills,
    /// worm crossings, and border skirmish hits; decays slowly via
    /// the faction sampler. High pressure amplifies border skirmish
    /// chances and makes cross-faction bridge rolls more likely
    /// between the rivals — feuds become sticky instead of uniform.
    pub rivalry: HashMap<(u8, u8), u16>,
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
    /// Active open-war declarations. Key is the canonical
    /// rivalry pair (min, max); value is the tick at which the
    /// war window expires. While a pair is at war, border
    /// skirmish chance gets an extra 3x multiplier on top of the
    /// rivalry pressure amp.
    pub wars: HashMap<(u8, u8), u64>,
    /// Faction currently holding dominance (≥ VICTORY_ALIVE_FRACTION
    /// of total alive nodes, or sole surviving C2). Cleared when
    /// the dominant faction drops below the threshold. Purely a
    /// readout — the sim never auto-ends; dominance is just a
    /// tracked state the UI and summary surface.
    pub current_dominant: Option<u8>,
    /// Cumulative count of distinct dominance declarations fired
    /// this run, so the summary can show 'F0 crowned 3 times'.
    pub dominance_shifts: u32,
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
    /// `WAR_DECLARATION_THRESHOLD` within the last
    /// `WAR_DURATION_TICKS` ticks.
    pub fn at_war(&self, a: u8, b: u8) -> bool {
        Self::rivalry_key(a, b)
            .and_then(|k| self.wars.get(&k).copied())
            .map(|exp| exp > self.tick)
            .unwrap_or(false)
    }

    /// Scan rivalry pairs for newly-crossed war declarations and
    /// promote them. A pair only declares once per rivalry lifetime
    /// — after a declaration fires, the rivalry has to fully decay
    /// below the threshold and re-climb to trigger another.
    fn maybe_declare_wars(&mut self) {
        let mut new_wars: Vec<(u8, u8)> = Vec::new();
        for (&(a, b), &pressure) in &self.rivalry {
            if pressure < WAR_DECLARATION_THRESHOLD {
                continue;
            }
            // Skip if already at war — avoids re-declaring every
            // sample period while the rivalry stays above threshold.
            if self.wars.contains_key(&(a, b)) {
                continue;
            }
            new_wars.push((a, b));
        }
        for (a, b) in new_wars {
            self.wars.insert((a, b), self.tick + WAR_DURATION_TICKS);
            self.push_log(format!(
                "✦ WAR ✦ F{} declares open hostilities on F{}",
                a, b
            ));
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

    /// Drop a patch wave at `origin`. Uses the same geometry as
    /// the heartbeat-driven waves so the visual/mechanic is
    /// identical; it's just triggered by a keybind instead of the
    /// timer. Used by the cursor-action hotkey 'p'.
    pub fn inject_patch_wave(&mut self, origin: (i16, i16)) {
        self.patch_waves.push(PatchWave { origin, radius: 0 });
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
        let life = self.cfg.wormhole_life_ticks;
        self.wormholes.push(Wormhole {
            a: origin,
            b: other,
            age: 0,
            life,
        });
        self.push_log("wormhole injected".to_string());
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

    /// Read the current war pressure between two factions. Zero if
    /// they've never tangled.
    pub fn rivalry_pressure(&self, a: u8, b: u8) -> u16 {
        Self::rivalry_key(a, b)
            .and_then(|k| self.rivalry.get(&k).copied())
            .unwrap_or(0)
    }

    /// Bump war pressure between two factions by `amount`, clamped
    /// to RIVALRY_CAP. No-op for self-pairs.
    pub fn bump_rivalry(&mut self, a: u8, b: u8, amount: u16) {
        if let Some(key) = Self::rivalry_key(a, b) {
            let entry = self.rivalry.entry(key).or_insert(0);
            *entry = entry.saturating_add(amount).min(RIVALRY_CAP);
        }
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

    /// True if factions `a` and `b` currently have a non-aggression
    /// alliance in effect.
    pub fn allied(&self, a: u8, b: u8) -> bool {
        if a == b {
            return true;
        }
        self.alliances.iter().any(|al| {
            al.expires_tick > self.tick
                && ((al.a == a && al.b == b) || (al.a == b && al.b == a))
        })
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
            alliances: Vec::new(),
            next_branch_id: 1,
            storm_until: 0,
            storm_since: 0,
            storm_dir: (0, 1),
            strain_names,
            faction_stats: vec![FactionStats::default(); count],
            mythic_pandemic_seen: false,
            activity_history: VecDeque::with_capacity(ACTIVITY_HISTORY_LEN),
            rivalry: HashMap::new(),
            outages: Vec::new(),
            partitions: Vec::new(),
            personas,
            faction_colors,
            wars: HashMap::new(),
            current_dominant: None,
            dominance_shifts: 0,
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
        self.maybe_alliance();
        self.maybe_border_skirmish();

        // Sample faction alive counts for the header sparkline.
        if self.tick.is_multiple_of(FACTION_SAMPLE_PERIOD) {
            self.sample_faction_history();
            // Reactive persona shifts based on current vs peak/avg.
            self.maybe_shift_personas();
            // Check for legendary-node promotions on the same cadence.
            self.maybe_promote_legendary();
            // Slow rivalry decay so old feuds eventually fade.
            self.rivalry.retain(|_, v| {
                *v = v.saturating_sub(2);
                *v > 0
            });
            // Promote any rivalry that crossed the war threshold
            // to an open war declaration.
            self.maybe_declare_wars();
            // Check for a dominance victory condition.
            self.maybe_declare_victory();
        }
        // Expire any wars whose windows have elapsed.
        self.wars.retain(|_, exp| *exp > self.tick);

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

