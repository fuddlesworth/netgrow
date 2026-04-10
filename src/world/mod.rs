use std::collections::{HashMap, HashSet, VecDeque};

use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

use crate::routing;

mod cascade;
mod config;
mod types;
mod virus;
pub use config::*;
pub use types::*;

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
            ddos_waves: Vec::new(),
            wormholes: Vec::new(),
            alliances: Vec::new(),
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
        self.maybe_ddos();
        self.advance_ddos_waves();
        self.maybe_wormhole();
        self.advance_wormholes();
        self.maybe_assimilate();
        self.maybe_alliance();
        self.maybe_border_skirmish();

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
        // Roll once per attempt: is this a cross-faction bridge?
        // Allied factions stay peaceful — their bridges form normally
        // within same-faction only.
        let allow_cross_faction = self.cfg.cross_faction_bridge_chance > 0.0
            && self.rng.gen_bool(self.cfg.cross_faction_bridge_chance as f64);
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
                // Same-faction by default; cross-faction when the roll
                // allows and we explicitly want a different faction.
                // Allied factions stay peaceful — don't cross-bridge
                // during an alliance.
                let same_faction = self.nodes[b].faction == a_faction;
                if allow_cross_faction {
                    if same_faction {
                        return false;
                    }
                    if self.allied(a_faction, self.nodes[b].faction) {
                        return false;
                    }
                } else if !same_faction {
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
        if self.nodes[a].faction != self.nodes[b].faction {
            self.push_log(format!(
                "bridge F{}↔F{} CROSS-FACTION",
                self.nodes[a].faction, self.nodes[b].faction
            ));
        } else {
            self.push_log(format!(
                "bridge {}↔{} established",
                self.nodes[a].branch_id, self.nodes[b].branch_id
            ));
        }
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
pub(super) fn octet_pair(pos: (i16, i16)) -> (u8, u8) {
    ((pos.0 as u32 & 0xff) as u8, (pos.1 as u32 & 0xff) as u8)
}

/// Pretty-printed 10.0.X.Y address derived from a mesh cell. Used by
/// the log lines and inspector panel so every node has a stable,
/// shareable identifier.
pub fn node_ip(pos: (i16, i16)) -> String {
    let (a, b) = octet_pair(pos);
    format!("10.0.{}.{}", a, b)
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

    fn maybe_alliance(&mut self) {
        // Expire any done alliances first.
        let now = self.tick;
        let prev_len = self.alliances.len();
        self.alliances.retain(|al| al.expires_tick > now);
        if self.alliances.len() < prev_len {
            self.push_log("alliance dissolved".to_string());
        }
        if !self.roll_periodic(self.cfg.alliance_period, self.cfg.alliance_chance) {
            return;
        }
        if self.c2_nodes.len() < 2 {
            return;
        }
        // Pick two distinct faction ids.
        let n = self.c2_nodes.len() as u8;
        let a = self.rng.gen_range(0..n);
        let mut b = self.rng.gen_range(0..n);
        while b == a && n > 1 {
            b = self.rng.gen_range(0..n);
        }
        if a == b {
            return;
        }
        // Skip if already allied.
        if self.allied(a, b) {
            return;
        }
        let expires_tick = now + self.cfg.alliance_duration;
        self.alliances.push(Alliance { a, b, expires_tick });
        self.push_log(format!("alliance F{} ↔ F{} signed", a, b));
    }

    /// Border skirmishes: periodic low-probability hits on nodes that
    /// sit near an enemy-faction neighbor. Visible as scattered
    /// shielded/LOST lines at faction frontiers during long runs.
    fn maybe_border_skirmish(&mut self) {
        if !self.roll_periodic(self.cfg.border_skirmish_period, 1.0) {
            return;
        }
        if self.c2_nodes.len() < 2 {
            return;
        }
        let radius = self.cfg.border_skirmish_radius;
        let chance = self.cfg.border_skirmish_chance;
        if chance <= 0.0 {
            return;
        }
        // Build a snapshot of faction positions so we can scan without
        // aliasing self.
        let positions: Vec<(NodeId, (i16, i16), u8)> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                if matches!(n.state, State::Alive) && !self.is_c2(i) {
                    Some((i, n.pos, n.faction))
                } else {
                    None
                }
            })
            .collect();
        let pwned_flash = self.cfg.pwned_flash_ticks;
        let mut victims: Vec<NodeId> = Vec::new();
        for &(id, pos, faction) in &positions {
            let near_enemy = positions.iter().any(|&(_, p, f)| {
                f != faction
                    && !self.allied(f, faction)
                    && (p.0 - pos.0).abs().max((p.1 - pos.1).abs()) <= radius
            });
            if !near_enemy {
                continue;
            }
            if self.rng.gen_bool(chance as f64) {
                victims.push(id);
            }
        }
        for id in victims {
            let pos = self.nodes[id].pos;
            let node = &mut self.nodes[id];
            if node.hardened {
                node.hardened = false;
                node.heartbeats = 0;
                node.shield_flash = 6;
                self.log_node(pos, "skirmish shielded");
            } else {
                node.state = State::Pwned {
                    ticks_left: pwned_flash,
                };
                self.log_node(pos, "skirmish LOST");
            }
        }
    }

    /// Faction extinction mechanic. When a faction drops below
    /// assimilation_threshold alive nodes and another faction has at
    /// least assimilation_dominance alive nodes, the weak faction's
    /// remaining nodes flip to the strongest faction's color and its
    /// C2 is marked dead — visible as a dramatic color swap + mythic
    /// log line.
    fn maybe_assimilate(&mut self) {
        if !self.roll_periodic(self.cfg.assimilation_period, 1.0) {
            return;
        }
        if self.c2_nodes.len() < 2 {
            return;
        }
        // Count alive per faction.
        let mut counts = vec![0usize; self.faction_stats.len()];
        for n in &self.nodes {
            if matches!(n.state, State::Alive) {
                if let Some(slot) = counts.get_mut(n.faction as usize) {
                    *slot += 1;
                }
            }
        }
        let weak_threshold = self.cfg.assimilation_threshold;
        let strong_threshold = self.cfg.assimilation_dominance;
        // Find the strongest faction that meets the dominance bar.
        let mut strongest: Option<(usize, usize)> = None;
        for (i, &c) in counts.iter().enumerate() {
            if c >= strong_threshold
                && strongest.map(|(_, sc)| c > sc).unwrap_or(true)
            {
                strongest = Some((i, c));
            }
        }
        let Some((strong_idx, _)) = strongest else {
            return;
        };
        // Find a weak faction that isn't the strong one.
        let weak_idx = counts
            .iter()
            .enumerate()
            .find(|(i, &c)| *i != strong_idx && c > 0 && c <= weak_threshold)
            .map(|(i, _)| i);
        let Some(weak_idx) = weak_idx else {
            return;
        };
        // Flip all the weak faction's alive nodes to the strong
        // faction. The weak faction's C2 gets marked dead.
        let new_faction = strong_idx as u8;
        let weak_c2 = self.c2_nodes[weak_idx];
        self.nodes[weak_c2].state = State::Dead;

        // Snapshot strong-faction alive positions up front so we can
        // reparent each absorbed node to its nearest strong neighbor.
        // Without this step the flipped nodes keep their old parent
        // (the now-dead weak C2), which immediately isolates them
        // from the strong C2's reachability tree and dooms them.
        let strong_positions: Vec<(NodeId, (i16, i16))> = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                n.faction == new_faction && matches!(n.state, State::Alive)
            })
            .map(|(i, n)| (i, n.pos))
            .collect();

        // Collect absorbed node ids first (can't flip while borrowing).
        let absorbed: Vec<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                n.faction as usize == weak_idx && matches!(n.state, State::Alive)
            })
            .map(|(i, _)| i)
            .collect();

        let mut flipped = 0u32;
        for id in absorbed {
            let pos = self.nodes[id].pos;
            // Nearest strong-faction alive node becomes the new parent.
            // Fall back to the strong C2 if no other strong nodes exist.
            let new_parent = strong_positions
                .iter()
                .min_by_key(|(_, p)| (p.0 - pos.0).abs().max((p.1 - pos.1).abs()))
                .map(|(pid, _)| *pid)
                .unwrap_or(self.c2_nodes[strong_idx]);
            self.nodes[id].faction = new_faction;
            self.nodes[id].parent = Some(new_parent);
            flipped += 1;
        }

        self.push_log(format!(
            "✦ MYTHIC ✦ ASSIMILATION — F{} absorbed by F{} ({} hosts)",
            weak_idx, strong_idx, flipped
        ));
    }

    fn maybe_wormhole(&mut self) {
        if !self.roll_periodic(self.cfg.wormhole_period, self.cfg.wormhole_chance) {
            return;
        }
        // Pick two distinct alive nodes to link.
        let alive: Vec<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                if matches!(n.state, State::Alive) && !self.is_c2(i) {
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
        let mut b = alive[self.rng.gen_range(0..alive.len())];
        while b == a {
            b = alive[self.rng.gen_range(0..alive.len())];
        }
        let a_pos = self.nodes[a].pos;
        let b_pos = self.nodes[b].pos;
        let life = self.cfg.wormhole_life_ticks;
        self.wormholes.push(Wormhole {
            a: a_pos,
            b: b_pos,
            age: 0,
            life,
        });
        let (oa1, oa2) = octet_pair(a_pos);
        let (ob1, ob2) = octet_pair(b_pos);
        self.push_log(format!(
            "wormhole 10.0.{}.{} ↔ 10.0.{}.{}",
            oa1, oa2, ob1, ob2
        ));
    }

    fn advance_wormholes(&mut self) {
        for wh in self.wormholes.iter_mut() {
            wh.age = wh.age.saturating_add(1);
        }
        self.wormholes.retain(|w| w.age < w.life);
    }

    fn maybe_ddos(&mut self) {
        if !self.roll_periodic(self.cfg.ddos_period, self.cfg.ddos_chance) {
            return;
        }
        // Pick a random edge to originate from and sweep toward the
        // opposite side.
        let edge = self.rng.gen_range(0..4u8);
        let (horizontal, pos, direction) = match edge {
            0 => (true, 0, 1),                    // top, moving down
            1 => (true, self.bounds.1 - 1, -1),   // bottom, moving up
            2 => (false, 0, 1),                   // left, moving right
            _ => (false, self.bounds.0 - 1, -1),  // right, moving left
        };
        self.ddos_waves.push(DdosWave {
            pos,
            horizontal,
            direction,
            age: 0,
        });
        self.push_log("⚡ DDOS wave inbound".to_string());
    }

    fn advance_ddos_waves(&mut self) {
        if self.ddos_waves.is_empty() {
            return;
        }
        let stun = self.cfg.ddos_stun_ticks;
        let bounds = self.bounds;
        let mut keep: Vec<DdosWave> = Vec::with_capacity(self.ddos_waves.len());
        for mut wave in std::mem::take(&mut self.ddos_waves) {
            // Apply stun to any alive node whose position coincides
            // with the current wave line.
            for n in self.nodes.iter_mut() {
                if !matches!(n.state, State::Alive) {
                    continue;
                }
                let hit = if wave.horizontal {
                    n.pos.1 == wave.pos
                } else {
                    n.pos.0 == wave.pos
                };
                if hit {
                    // Cap stun accumulation at DDOS_MAX_STUN so overlapping
                    // waves can't effectively disable a node forever.
                    const DDOS_MAX_STUN: u16 = 500;
                    n.role_cooldown = n.role_cooldown.saturating_add(stun).min(DDOS_MAX_STUN);
                    n.scan_pulse = n.scan_pulse.max(3);
                }
            }
            wave.age = wave.age.saturating_add(1);
            wave.pos += wave.direction;
            let in_bounds = if wave.horizontal {
                (0..bounds.1).contains(&wave.pos)
            } else {
                (0..bounds.0).contains(&wave.pos)
            };
            if in_bounds {
                keep.push(wave);
            }
        }
        self.ddos_waves = keep;
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
        // Roll for a new storm only when one isn't already active.
        if self.storm_until > 0 {
            return;
        }
        if !self.roll_periodic(self.cfg.storm_period, self.cfg.storm_chance) {
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
            // Source of the worm is the opposite endpoint from the
            // target — used to enforce alliance non-aggression below.
            // Alliance blocks worm crossings between DIFFERENT factions
            // only. Same-faction worms (where source and target share a
            // faction) always deliver.
            let blocked_by_alliance = |w: &World, src: u8, dst: u8| -> bool {
                src != dst && w.allied(src, dst)
            };
            if worm.outbound_from_a {
                let next = worm.pos as usize + 1;
                if next >= link_len {
                    let target = link_b;
                    let src = self.nodes[link_a].faction;
                    let dst = self.nodes[target].faction;
                    if !c2_set.contains(&target)
                        && matches!(self.nodes[target].state, State::Alive)
                        && self.nodes[target].infection.is_none()
                        && !blocked_by_alliance(self, src, dst)
                    {
                        arrivals.push((target, worm.strain, self.nodes[target].pos));
                    }
                    continue;
                }
                worm.pos = next as u16;
            } else {
                if worm.pos == 0 {
                    let target = link_a;
                    let src = self.nodes[link_b].faction;
                    let dst = self.nodes[target].faction;
                    if !c2_set.contains(&target)
                        && matches!(self.nodes[target].state, State::Alive)
                        && self.nodes[target].infection.is_none()
                        && !blocked_by_alliance(self, src, dst)
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

