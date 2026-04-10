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
}

#[derive(Clone, Debug)]
pub struct Link {
    pub a: NodeId,
    pub b: NodeId,
    pub path: Vec<(i16, i16)>,
    pub drawn: u16,
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
        let c2_node = Node {
            pos: center,
            parent: None,
            state: State::Alive,
            born: 0,
            pulse: 0,
            dying_in: 0,
        };
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
        }
    }

    pub fn tick(&mut self, bounds: (i16, i16)) {
        self.bounds = bounds;

        self.try_spawn();
        self.advance_links();
        self.heartbeat();
        self.advance_pwned_and_loss();
        self.advance_dying();

        self.tick += 1;
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
        let dir = DIRS[self.rng.gen_range(0..DIRS.len())];
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

        let new_id = self.nodes.len();
        self.nodes.push(Node {
            pos: cand,
            parent: Some(parent_id),
            state: State::Alive,
            born: self.tick,
            pulse: 0,
            dying_in: 0,
        });
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
            for n in self.nodes.iter_mut() {
                if matches!(n.state, State::Alive) {
                    n.pulse = 2;
                }
            }
            self.push_log(format!("beacon sweep @ t={}", self.tick));
        } else {
            for n in self.nodes.iter_mut() {
                if n.pulse > 0 {
                    n.pulse -= 1;
                }
            }
        }
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
            self.schedule_subtree_death(id);
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
                self.nodes[victim].state = State::Pwned {
                    ticks_left: self.cfg.pwned_flash_ticks,
                };
                let pos = self.nodes[victim].pos;
                let a = (pos.0 as u32 & 0xff) as u8;
                let b = (pos.1 as u32 & 0xff) as u8;
                self.push_log(format!("node 10.0.{}.{} LOST", a, b));
            }
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
        w.nodes.push(Node {
            pos: (10, 10),
            parent: Some(w.c2),
            state: State::Alive,
            born: 0,
            pulse: 0,
            dying_in: 0,
        });
        let b = w.nodes.len();
        w.nodes.push(Node {
            pos: (12, 10),
            parent: Some(a),
            state: State::Alive,
            born: 0,
            pulse: 0,
            dying_in: 0,
        });
        let c = w.nodes.len();
        w.nodes.push(Node {
            pos: (14, 10),
            parent: Some(b),
            state: State::Alive,
            born: 0,
            pulse: 0,
            dying_in: 0,
        });
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
    fn tick_runs_without_panic_and_grows() {
        let mut w = World::new(42, (80, 24), Config::default());
        for _ in 0..500 {
            w.tick((80, 24));
        }
        assert!(w.nodes.len() > 1);
    }
}
