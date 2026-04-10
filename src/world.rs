use std::collections::{HashMap, HashSet, VecDeque};

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

use crate::routing;

pub type NodeId = usize;

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
}

#[derive(Clone, Debug)]
pub struct Ping {
    pub origin: (i16, i16),
    pub born: u64,
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
}

impl Default for RoleWeights {
    fn default() -> Self {
        Self {
            relay: 0.72,
            scanner: 0.15,
            exfil: 0.1,
            honeypot: 0.03,
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
            virus_cure_resist: 3,
            virus_seed_rate: 0.004,
            worm_spawn_rate: 0.04,
            patch_wave_radius: 10,
            mutate_rate: 0.0008,
            mutate_min_age: 400,
            zero_day_period: 2000,
            zero_day_chance: 0.4,
        }
    }
}

pub struct World {
    pub nodes: Vec<Node>,
    pub links: Vec<Link>,
    pub c2: NodeId,
    pub rng: ChaCha8Rng,
    pub tick: u64,
    pub occupied: HashSet<(i16, i16)>,
    pub logs: VecDeque<String>,
    pub bounds: (i16, i16),
    pub cfg: Config,
    pub pings: Vec<Ping>,
    pub packets: Vec<Packet>,
    pub worms: Vec<Worm>,
    pub patch_waves: Vec<PatchWave>,
    pub next_branch_id: u16,
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

#[derive(Clone, Copy, Debug, Default)]
pub struct WorldStats {
    pub alive: usize,
    pub pwned: usize,
    pub dead: usize,
    pub dying: usize,
    pub branches: usize,
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
        let rng = ChaCha8Rng::seed_from_u64(seed);
        let center = (bounds.0 / 2, bounds.1 / 2);
        let c2_node = Node::fresh(center, None, 0, Role::Relay, 0);
        let mut occupied = HashSet::new();
        occupied.insert(center);
        let mut logs = VecDeque::new();
        logs.push_back(format!("c2 online @ {},{}", center.0, center.1));
        Self {
            nodes: vec![c2_node],
            links: Vec::new(),
            c2: 0,
            rng,
            tick: 0,
            occupied,
            logs,
            bounds,
            cfg,
            pings: Vec::new(),
            packets: Vec::new(),
            worms: Vec::new(),
            patch_waves: Vec::new(),
            next_branch_id: 1,
        }
    }

    pub fn tick(&mut self, bounds: (i16, i16)) {
        self.bounds = bounds;

        self.try_spawn();
        self.advance_links();
        self.advance_pings();
        self.advance_packets();
        self.advance_worms();
        self.advance_patch_waves();
        self.heartbeat();
        self.advance_role_cooldowns();
        self.maybe_mutate();
        self.maybe_zero_day();
        self.fire_scanner_pings();
        self.fire_exfil_packets();
        self.advance_infections();
        self.maybe_spawn_worms();
        self.maybe_seed_infection();
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
                if i != self.c2
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
        });
        self.push_log(format!(
            "bridge {}↔{} established",
            self.nodes[a].branch_id, self.nodes[b].branch_id
        ));
    }

    fn roll_role(&mut self) -> Role {
        let w = &self.cfg.role_weights;
        let total = w.relay + w.scanner + w.exfil + w.honeypot;
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
        Role::Honeypot
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
        if !self.rng.gen_bool(self.cfg.p_spawn as f64) {
            return;
        }

        // Weighted pick over Alive nodes, favoring recent births.
        let now = self.tick;
        let mut candidates: Vec<(NodeId, f32)> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| match n.state {
                State::Alive => {
                    let age = (now - n.born) as f32;
                    Some((i, 1.0 / (1.0 + age * 0.1)))
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

        // Branch id: first-hop children of C2 each spawn a fresh branch so
        // distinct colonies get distinct colors; deeper children inherit.
        let branch_id = if parent_id == self.c2 {
            self.alloc_branch_id()
        } else {
            self.nodes[parent_id].branch_id
        };
        let role = self.roll_role();

        let new_id = self.nodes.len();
        self.nodes
            .push(Node::fresh(cand, Some(parent_id), self.tick, role, branch_id));
        self.occupied.insert(cand);
        self.links.push(Link {
            a: parent_id,
            b: new_id,
            path,
            drawn: 0,
            kind: LinkKind::Parent,
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
            // Emit a patch wave from C2 alongside the beacon pulse.
            let c2_pos = self.nodes[self.c2].pos;
            self.patch_waves.push(PatchWave {
                origin: c2_pos,
                radius: 0,
            });
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
                let (a, b) = octet_pair(pos);
                self.push_log(format!("node 10.0.{}.{} hardened", a, b));
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
        }
    }

    fn fire_scanner_pings(&mut self) {
        let period = self.cfg.scanner_ping_period;
        let now = self.tick;
        let mut new_pings: Vec<Ping> = Vec::new();
        // Pick directions up front so we don't alias the rng borrow.
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
        for id in scanner_ids {
            let dir_idx = self.rng.gen_range(0..DIRS.len());
            let (dx, dy) = DIRS[dir_idx];
            let n = &mut self.nodes[id];
            n.role_cooldown = period;
            n.last_ping_dir = Some((dx as i8, dy as i8));
            n.last_ping_tick = now;
            new_pings.push(Ping {
                origin: n.pos,
                born: now,
            });
        }
        self.pings.extend(new_pings);
    }

    fn advance_pings(&mut self) {
        let now = self.tick;
        self.pings.retain(|p| now.saturating_sub(p.born) < 4);
        if self.pings.len() > 64 {
            let drop = self.pings.len() - 64;
            self.pings.drain(0..drop);
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

    fn advance_packets(&mut self) {
        if self.packets.is_empty() {
            return;
        }
        let inbound = self.build_inbound_links();

        let mut keep: Vec<Packet> = Vec::with_capacity(self.packets.len());
        for mut pkt in std::mem::take(&mut self.packets) {
            let link = &self.links[pkt.link_id];
            let a_state = self.nodes[link.a].state;
            let b_state = self.nodes[link.b].state;
            let a_dying = self.nodes[link.a].dying_in > 0;
            let b_dying = self.nodes[link.b].dying_in > 0;
            if matches!(a_state, State::Dead)
                || matches!(b_state, State::Dead)
                || a_dying
                || b_dying
            {
                continue; // drop packet; route is compromised
            }
            if pkt.pos == 0 {
                // Reached the parent end of this link. Hop to the parent's
                // own inbound link, or drop if parent is C2.
                let parent_id = link.a;
                if parent_id == self.c2 {
                    continue; // delivered
                }
                if let Some(&next_link) = inbound.get(&parent_id) {
                    let next = &self.links[next_link];
                    if next.path.is_empty() {
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
    }

    fn advance_worms(&mut self) {
        if self.worms.is_empty() {
            return;
        }
        let cure_resist = self.cfg.virus_cure_resist;
        let c2 = self.c2;
        let mut keep: Vec<Worm> = Vec::with_capacity(self.worms.len());
        let mut arrivals: Vec<(NodeId, u8, (i16, i16))> = Vec::new();
        for mut worm in std::mem::take(&mut self.worms) {
            let link = &self.links[worm.link_id];
            // Drop the worm if its carrier link is compromised.
            let a_node = &self.nodes[link.a];
            let b_node = &self.nodes[link.b];
            if matches!(a_node.state, State::Dead)
                || matches!(b_node.state, State::Dead)
                || a_node.dying_in > 0
                || b_node.dying_in > 0
            {
                continue;
            }
            if worm.outbound_from_a {
                let next = worm.pos as usize + 1;
                if next >= link.path.len() {
                    let target = link.b;
                    if target != c2
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
                    let target = link.a;
                    if target != c2
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
            self.push_log(format!("worm delivered strain {} @ 10.0.{}.{}", strain, a, b));
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
                if i == self.c2 || !matches!(n.state, State::Alive) {
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
            if target == self.c2 {
                continue;
            }
            if !matches!(self.nodes[target].state, State::Alive) {
                continue;
            }
            if self.nodes[target].infection.is_some() {
                continue;
            }
            let pos = if from_a {
                0
            } else {
                link.path.len().saturating_sub(1) as u16
            };
            self.worms.push(Worm {
                link_id,
                pos,
                outbound_from_a: from_a,
                strain,
            });
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
        let mut cured: Vec<(i16, i16)> = Vec::new();
        for n in self.nodes.iter_mut() {
            if n.infection.is_none() {
                continue;
            }
            for &(ox, oy, r) in &geo {
                let dist = (n.pos.0 - ox).abs().max((n.pos.1 - oy).abs());
                // Hit by the wave front or the trailing cell (annulus of
                // width 2) so fast-growing rings don't skip over nodes.
                if dist == r || dist == r - 1 {
                    let Some(inf) = n.infection.as_mut() else {
                        break;
                    };
                    if inf.cure_resist <= 1 {
                        cured.push(n.pos);
                        n.infection = None;
                        break;
                    } else {
                        inf.cure_resist -= 1;
                    }
                }
            }
        }
        self.patch_waves.retain(|w| w.radius <= max_r);
        for pos in cured {
            let (a, b) = octet_pair(pos);
            self.push_log(format!("node 10.0.{}.{} cured", a, b));
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
        let c2 = self.c2;
        let adj = self.live_adjacency();
        let mut newly_infected: Vec<(NodeId, u8)> = Vec::new();
        if spread_rate > 0.0 {
            for (id, n) in self.nodes.iter().enumerate() {
                if id == c2 {
                    continue;
                }
                if !matches!(n.state, State::Alive) || n.infection.is_some() {
                    continue;
                }
                let Some(neighbors) = adj.get(&id) else {
                    continue;
                };
                let mut tally: [u32; 8] = [0; 8];
                let mut infected_count: u32 = 0;
                for &m in neighbors {
                    if let Some(inf) = self.nodes[m].infection {
                        if !matches!(inf.stage, InfectionStage::Incubating) {
                            tally[(inf.strain as usize) & 7] += 1;
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
            let node = &mut self.nodes[id];
            node.infection = None;
            node.state = State::Pwned {
                ticks_left: pwned_flash,
            };
            let (a, b) = octet_pair(node.pos);
            self.push_log(format!("node 10.0.{}.{} necrotic", a, b));
        }

        for pos in newly_active {
            let (a, b) = octet_pair(pos);
            self.push_log(format!("node 10.0.{}.{} symptomatic", a, b));
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
                if i != self.c2 && matches!(n.state, State::Alive) {
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
        let strain = self.rng.gen_range(0..8u8);
        let cure_resist = self.cfg.virus_cure_resist;
        self.nodes[id].infection = Some(Infection::seeded(strain, cure_resist));
        let (a, b) = octet_pair(self.nodes[id].pos);
        self.push_log(format!("strain {} detected at 10.0.{}.{}", strain, a, b));
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
                if i == self.c2 {
                    return None;
                }
                if !matches!(n.state, State::Alive) {
                    return None;
                }
                if n.infection.is_some() {
                    return None;
                }
                if n.role == Role::Honeypot {
                    return None; // honeypots hide; mutation would blow cover
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
                Role::Honeypot => continue,
            };
            // Pick uniformly from the first two (the third is the sentinel).
            let new_role = choices[self.rng.gen_range(0..2)];
            self.nodes[id].role = new_role;
            self.nodes[id].mutated_flash = 6;
            let (a, b) = octet_pair(self.nodes[id].pos);
            let name = match new_role {
                Role::Relay => "relay",
                Role::Scanner => "scanner",
                Role::Exfil => "exfil",
                Role::Honeypot => "honeypot",
            };
            self.push_log(format!("node 10.0.{}.{} mutated → {}", a, b, name));
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
        if alive_count < 10 {
            return;
        }
        if !self.rng.gen_bool(self.cfg.zero_day_chance as f64) {
            return;
        }
        // Pick event type.
        let roll = self.rng.gen::<f32>();
        if roll < 0.6 {
            self.zero_day_outbreak();
        } else if roll < 0.9 {
            self.zero_day_emergency_patch();
        } else {
            self.zero_day_immune_breakthrough();
        }
    }

    fn zero_day_outbreak(&mut self) {
        let strain = self.rng.gen_range(0..8u8);
        // Infect 3-5 random alive nodes with a high cure_resist strain.
        let count = self.rng.gen_range(3..=5);
        let cure_resist = self.cfg.virus_cure_resist.saturating_mul(2);
        let candidates: Vec<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                if i != self.c2
                    && matches!(n.state, State::Alive)
                    && n.infection.is_none()
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
        let mut hit = 0u32;
        for _ in 0..count {
            let id = candidates[self.rng.gen_range(0..candidates.len())];
            if self.nodes[id].infection.is_none() {
                self.nodes[id].infection = Some(Infection::seeded(strain, cure_resist));
                hit += 1;
            }
        }
        self.push_log(format!(
            "ZERO-DAY: strain {} outbreak — {} hosts infected",
            strain, hit
        ));
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
            "ZERO-DAY: emergency patch deployed — {} hosts cleared",
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
            "ZERO-DAY: immune breakthrough — {} strains entrenched",
            boosted
        ));
    }

    /// Infect a random Alive non-C2 node with a fresh strain. Used by the
    /// `i` keybinding and by tests.
    pub fn inject_infection(&mut self) -> Option<NodeId> {
        let candidates: Vec<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                if i != self.c2
                    && matches!(n.state, State::Alive)
                    && n.infection.is_none()
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
        let strain = self.rng.gen_range(0..8u8);
        let cure_resist = self.cfg.virus_cure_resist;
        self.nodes[id].infection = Some(Infection::seeded(strain, cure_resist));
        let (a, b) = octet_pair(self.nodes[id].pos);
        self.push_log(format!("INJECTED strain {} @ 10.0.{}.{}", strain, a, b));
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
                if let Some(parent) = self.nodes[id].parent {
                    if parent != self.c2 {
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
        if self.rng.gen_bool(self.cfg.p_loss as f64) {
            let alive_ids: Vec<NodeId> = self
                .nodes
                .iter()
                .enumerate()
                .filter_map(|(i, n)| {
                    if i != self.c2 && matches!(n.state, State::Alive) {
                        Some(i)
                    } else {
                        None
                    }
                })
                .collect();
            if !alive_ids.is_empty() {
                let victim = alive_ids[self.rng.gen_range(0..alive_ids.len())];
                let node = &mut self.nodes[victim];
                let (a, b) = octet_pair(node.pos);

                if node.hardened {
                    // Reinforcement: consume the shield instead of pwning.
                    node.hardened = false;
                    node.heartbeats = 0;
                    node.shield_flash = 6;
                    self.push_log(format!("node 10.0.{}.{} shielded", a, b));
                } else if node.role == Role::Honeypot {
                    node.honey_tripped = true;
                    node.honey_reveal = 2;
                    node.state = State::Pwned {
                        ticks_left: self.cfg.pwned_flash_ticks,
                    };
                    self.push_log(format!("HONEYPOT 10.0.{}.{} TRIPPED", a, b));
                } else {
                    node.state = State::Pwned {
                        ticks_left: self.cfg.pwned_flash_ticks,
                    };
                    self.push_log(format!("node 10.0.{}.{} LOST", a, b));
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
    /// diff over parent + fully-drawn cross edges, so nodes with a live
    /// alternate route to C2 survive.
    fn compute_cascade(&self, root: NodeId) -> Vec<(NodeId, u8)> {
        let adj = self.live_adjacency();
        let reach_with = self.bfs_reachable(self.c2, &adj, None);
        let reach_without = self.bfs_reachable(self.c2, &adj, Some(root));
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
            self.nodes[*id].state = State::Dead;
        }
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
        for link in &self.links {
            if dead.contains(&link.a) || dead.contains(&link.b) {
                for c in &link.path {
                    if *c != self.nodes[self.c2].pos {
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
        self.logs.push_back(s);
        while self.logs.len() > self.cfg.log_cap {
            self.logs.pop_front();
        }
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
            .push(Node::fresh((10, 10), Some(w.c2), 0, Role::Relay, 1));
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
        assert!(matches!(w.nodes[w.c2].state, State::Alive));
    }

    #[test]
    fn hardened_node_resists_first_pwn() {
        let mut w = World::new(7, (80, 30), Config::default());
        w.cfg.p_spawn = 0.0;
        let id = w.nodes.len();
        let mut n = Node::fresh((10, 10), Some(w.c2), 0, Role::Relay, 1);
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
            .push(Node::fresh((30, 10), Some(w.c2), 0, Role::Relay, a));
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
            .push(Node::fresh((10, 10), Some(w.c2), 0, Role::Relay, 1));
        let b = w.nodes.len();
        w.nodes
            .push(Node::fresh((14, 10), Some(a), 0, Role::Exfil, 1));
        // Manufacture links with full paths marked drawn.
        let path_ca: Vec<(i16, i16)> =
            (w.nodes[w.c2].pos.0..=10).map(|x| (x, 10)).collect();
        let len_ca = path_ca.len() as u16;
        w.links.push(Link {
            a: w.c2,
            b: a,
            path: path_ca,
            drawn: len_ca,
            kind: LinkKind::Parent,
        });
        let path_ab: Vec<(i16, i16)> = (10..=14).map(|x| (x, 10)).collect();
        let len_ab = path_ab.len() as u16;
        w.links.push(Link {
            a,
            b,
            path: path_ab,
            drawn: len_ab,
            kind: LinkKind::Parent,
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
            .push(Node::fresh((20, 10), Some(w.c2), 0, Role::Relay, 1));
        let c = w.nodes.len();
        w.nodes
            .push(Node::fresh((30, 10), Some(w.c2), 0, Role::Relay, 2));
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
        let mut n = Node::fresh((10, 10), Some(w.c2), 0, Role::Relay, 1);
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
            .push(Node::fresh((20, 10), Some(w.c2), 0, Role::Relay, 1));
        w.nodes
            .push(Node::fresh((25, 12), Some(w.c2), 0, Role::Relay, 2));
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
            .push(Node::fresh((20, 10), Some(w.c2), 0, Role::Relay, 1));
        w.nodes
            .push(Node::fresh((25, 12), Some(w.c2), 0, Role::Relay, 1));
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
            .push(Node::fresh((10, 10), Some(w.c2), 0, Role::Relay, 1));
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
            .push(Node::fresh((10, 10), Some(w.c2), 0, Role::Relay, 1));
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
        assert!(w.nodes[w.c2].infection.is_none(), "C2 must stay clean");
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
        let c2_pos = w.nodes[w.c2].pos;
        let a = w.nodes.len();
        w.nodes.push(Node::fresh(
            (c2_pos.0 + 3, c2_pos.1),
            Some(w.c2),
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
            .push(Node::fresh((10, 10), Some(w.c2), 0, Role::Relay, 1));
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
            .push(Node::fresh((10, 10), Some(w.c2), 0, Role::Relay, 1));
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
            .push(Node::fresh((10, 10), Some(w.c2), 0, Role::Honeypot, 1));
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
            .push(Node::fresh((10, 10), Some(w.c2), 0, Role::Relay, 1));
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
    fn tick_runs_without_panic_and_grows() {
        let mut w = World::new(42, (80, 24), Config::default());
        for _ in 0..500 {
            w.tick((80, 24));
        }
        assert!(w.nodes.len() > 1);
    }
}
