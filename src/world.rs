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

#[derive(Clone, Debug)]
pub struct Node {
    pub pos: (i16, i16),
    pub parent: Option<NodeId>,
    pub state: State,
    pub born: u64,
    pub pulse: u8,
    /// >0 means scheduled to die; render as red ✕ until it hits 0, then flip to Dead.
    /// Set via schedule_subtree_death with a delay proportional to distance from the
    /// pwned root, producing a visible red ripple through the subtree.
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
}

impl Node {
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
        }
    }
}

#[derive(Clone, Debug)]
pub struct Link {
    pub a: NodeId,
    pub b: NodeId,
    pub path: Vec<(i16, i16)>,
    pub drawn: u16,
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
            hardened_after_heartbeats: 4,
            honeypot_cascade_mult: 3.0,
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

impl World {
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
            next_branch_id: 1,
        }
    }

    pub fn tick(&mut self, bounds: (i16, i16)) {
        self.bounds = bounds;

        self.try_spawn();
        self.advance_links();
        self.advance_pings();
        self.advance_packets();
        self.heartbeat();
        self.advance_role_cooldowns();
        self.fire_scanner_pings();
        self.fire_exfil_packets();
        self.advance_pwned_and_loss();
        self.advance_dying();

        self.tick += 1;
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
            if parent.role == Role::Scanner
                && parent.last_ping_dir.is_some()
                && self.tick.saturating_sub(parent.last_ping_tick) < ping_window
            {
                let (dx, dy) = parent.last_ping_dir.unwrap();
                (dx as i16, dy as i16)
            } else {
                DIRS[self.rng.gen_range(0..DIRS.len())]
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
        let step_amount: u16 = if self.tick % 2 == 0 { 1 } else { 2 };
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
        if self.tick > 0 && self.tick % self.cfg.heartbeat_period == 0 {
            let threshold = self.cfg.hardened_after_heartbeats;
            let mut newly_hardened: Vec<(i16, i16)> = Vec::new();
            for n in self.nodes.iter_mut() {
                if matches!(n.state, State::Alive) {
                    n.pulse = 2;
                    if n.heartbeats < 255 {
                        n.heartbeats += 1;
                    }
                    if !n.hardened && n.heartbeats >= threshold {
                        n.hardened = true;
                        newly_hardened.push(n.pos);
                    }
                }
            }
            self.push_log(format!("beacon sweep @ t={}", self.tick));
            for pos in newly_hardened {
                let a = (pos.0 as u32 & 0xff) as u8;
                let b = (pos.1 as u32 & 0xff) as u8;
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
                if matches!(n.state, State::Alive) && n.role == Role::Scanner && n.role_cooldown == 0 {
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
        // Index inbound link per node (parent → this node).
        let mut inbound: HashMap<NodeId, usize> = HashMap::new();
        for (li, link) in self.links.iter().enumerate() {
            inbound.insert(link.b, li);
        }
        let exfil_ids: Vec<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                if matches!(n.state, State::Alive)
                    && n.role == Role::Exfil
                    && n.role_cooldown == 0
                    && !n.honey_tripped
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
        // Build inbound index for parent-hops.
        let mut inbound: HashMap<NodeId, usize> = HashMap::new();
        for (li, link) in self.links.iter().enumerate() {
            inbound.insert(link.b, li);
        }

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
                        self.schedule_subtree_death_scaled(parent, self.cfg.honeypot_cascade_mult);
                        continue;
                    }
                }
                self.schedule_subtree_death_scaled(id, self.cfg.honeypot_cascade_mult);
            } else {
                self.schedule_subtree_death(id);
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
                let pos = node.pos;
                let a = (pos.0 as u32 & 0xff) as u8;
                let b = (pos.1 as u32 & 0xff) as u8;

                if node.hardened {
                    // Reinforcement: consume the shield instead of pwning.
                    node.hardened = false;
                    node.heartbeats = 0;
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

    /// Like schedule_subtree_death but each hop's delay is multiplied by `mult`,
    /// stretching the red wave out for a more theatrical kill (honeypot trap).
    pub fn schedule_subtree_death_scaled(&mut self, root: NodeId, mult: f32) {
        let mut children: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for (i, n) in self.nodes.iter().enumerate() {
            if let Some(p) = n.parent {
                children.entry(p).or_default().push(i);
            }
        }
        let mut queue: VecDeque<(NodeId, u8)> = VecDeque::new();
        queue.push_back((root, 0));
        let mut touched = 0u32;
        while let Some((id, distance)) = queue.pop_front() {
            let base = distance.saturating_mul(2).saturating_add(3) as f32;
            let delay = (base * mult).round().clamp(1.0, 255.0) as u8;
            if self.nodes[id].dying_in == 0 || self.nodes[id].dying_in > delay {
                self.nodes[id].dying_in = delay;
                touched += 1;
            }
            if let Some(cs) = children.get(&id) {
                for &c in cs {
                    queue.push_back((c, distance + 1));
                }
            }
        }
        if touched > 0 {
            self.push_log(format!("HONEYPOT cascade: {} hosts burning", touched));
        }
    }

    /// Stagger death through a subtree so the kill is visible as a red wave
    /// spreading outward from `root`. Nodes are left in their current state
    /// but tagged with `dying_in`; render reads that and paints red.
    /// NOTE: parent-based subtree walk. If the graph ever becomes a mesh
    /// (multiple parents), switch to a reachability check from c2.
    pub fn schedule_subtree_death(&mut self, root: NodeId) {
        let mut children: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for (i, n) in self.nodes.iter().enumerate() {
            if let Some(p) = n.parent {
                children.entry(p).or_default().push(i);
            }
        }
        let mut queue: VecDeque<(NodeId, u8)> = VecDeque::new();
        queue.push_back((root, 0));
        let mut touched = 0u32;
        while let Some((id, distance)) = queue.pop_front() {
            // 2 ticks per hop, plus a 3-tick lead so the flash is visible.
            let delay = distance.saturating_mul(2).saturating_add(3);
            if self.nodes[id].dying_in == 0 || self.nodes[id].dying_in > delay {
                self.nodes[id].dying_in = delay.max(1);
                touched += 1;
            }
            if let Some(cs) = children.get(&id) {
                for &c in cs {
                    queue.push_back((c, distance + 1));
                }
            }
        }
        if touched > 0 {
            self.push_log(format!("cascade: {} hosts burning", touched));
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
        w.schedule_subtree_death(a);
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
        });
        let path_ab: Vec<(i16, i16)> = (10..=14).map(|x| (x, 10)).collect();
        let len_ab = path_ab.len() as u16;
        w.links.push(Link {
            a,
            b,
            path: path_ab,
            drawn: len_ab,
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
    fn tick_runs_without_panic_and_grows() {
        let mut w = World::new(42, (80, 24), Config::default());
        for _ in 0..500 {
            w.tick((80, 24));
        }
        assert!(w.nodes.len() > 1);
    }
}
