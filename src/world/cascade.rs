//! Loss and cascade subsystem: pwn resolution, reachability diffs,
//! subtree death propagation, honeypot backdoors, and C2 resurrection.
//!
//! Split out of `world/mod.rs` so the core tick loop stays small. The
//! entry points called from the tick are `advance_pwned_and_loss` and
//! `advance_dying`; everything else is a private helper of the cascade
//! pipeline or a utility (`live_adjacency`) shared with the virus layer.

use std::collections::{HashMap, HashSet, VecDeque};

use rand::seq::SliceRandom;
use rand::Rng;

use crate::routing;

use super::{
    octet_pair, FactionStats, Link, LinkKind, NodeId, Role, State, World, GHOST_ECHO_TICKS,
};

impl World {
    pub(super) fn advance_pwned_and_loss(&mut self) {
        // Tick down existing Pwned nodes.
        let mut to_schedule: Vec<NodeId> = Vec::new();
        for (i, n) in self.meshes[0].nodes.iter_mut().enumerate() {
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
            if self.meshes[0].nodes[id].role == Role::Honeypot && self.meshes[0].nodes[id].honey_tripped {
                // Reveal backdoor cross-links before cascading so the
                // shortcuts are visible for a few ticks before the death
                // wave propagates outward from them.
                self.reveal_honeypot_backdoors(id);
                let cm = self.era_rules.cascade_mult;
                if let Some(parent) = self.meshes[0].nodes[id].parent {
                    if !self.is_c2(parent) {
                        self.schedule_subtree_death(
                            parent,
                            self.cfg.honeypot_cascade_mult * cm,
                        );
                        continue;
                    }
                }
                self.schedule_subtree_death(id, self.cfg.honeypot_cascade_mult * cm);
            } else {
                self.schedule_subtree_death(id, self.era_rules.cascade_mult);
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
        let effective_loss =
            (self.cfg.p_loss * loss_mult * self.era_rules.loss_mult).clamp(0.0, 1.0);
        if self.rng.gen_bool(effective_loss as f64) {
            let alive_ids: Vec<NodeId> = self
                .meshes[0]
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
                let pos = self.meshes[0].nodes[victim].pos;
                let node = &mut self.meshes[0].nodes[victim];

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
    pub(super) fn live_adjacency(&self) -> HashMap<NodeId, Vec<NodeId>> {
        let mut adj: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let traversable = |id: NodeId| -> bool {
            let n = &self.meshes[0].nodes[id];
            !matches!(n.state, State::Dead) && n.dying_in == 0
        };
        for (id, n) in self.meshes[0].nodes.iter().enumerate() {
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
        for link in &self.meshes[0].links {
            if link.kind != LinkKind::Cross {
                continue;
            }
            // Latent sleeper links don't carry reachability
            // until they've been activated.
            if link.latent {
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
    /// diff anchored on the root's own faction's C2. Adjacency is
    /// filtered to the root's faction so cross-faction bridges (which
    /// maybe_reconnect can now create via cross_faction_bridge_chance)
    /// don't let a cascade leak across borders.
    pub(super) fn compute_cascade(&self, root: NodeId) -> Vec<(NodeId, u8)> {
        let root_faction = self.meshes[0].nodes[root].faction;
        let full_adj = self.live_adjacency();
        // Same-faction-only view: drop edges where either endpoint
        // belongs to a different faction.
        let mut adj: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for (id, neighbors) in full_adj.iter() {
            if self.meshes[0].nodes[*id].faction != root_faction {
                continue;
            }
            let filtered: Vec<NodeId> = neighbors
                .iter()
                .copied()
                .filter(|&m| self.meshes[0].nodes[m].faction == root_faction)
                .collect();
            adj.insert(*id, filtered);
        }
        let faction = root_faction as usize;
        let anchor = self
            .meshes[0]
            .c2_nodes
            .get(faction)
            .copied()
            .unwrap_or(self.meshes[0].c2_nodes[0]);
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
    pub(super) fn reveal_honeypot_backdoors(&mut self, honey_id: NodeId) {
        let max = self.cfg.honeypot_backdoor_max;
        if max == 0 {
            return;
        }
        let radius = self.cfg.honeypot_backdoor_radius;
        let a_pos = self.meshes[0].nodes[honey_id].pos;
        let a_branch = self.meshes[0].nodes[honey_id].branch_id;
        let a_faction = self.meshes[0].nodes[honey_id].faction;

        // Collect nearby eligible targets.
        let mut candidates: Vec<NodeId> = self
            .meshes[0]
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
                let already = self.meshes[0].links.iter().any(|l| {
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

        // Routing blocks only on other node positions, not link
        // paths — so backdoor cross-links are free to cross over
        // existing wires just like spawn and reconnect links do.
        let mut occ: HashSet<(i16, i16)> = self
            .meshes[0]
            .nodes
            .iter()
            .map(|n| n.pos)
            .collect();
        occ.remove(&a_pos);

        let bounds = self.meshes[0].bounds;
        let mut revealed = 0u32;
        for &b in candidates.iter().take(take) {
            let b_pos = self.meshes[0].nodes[b].pos;
            occ.remove(&b_pos);
            let path = routing::route_link(a_pos, b_pos, &occ, bounds, &mut self.rng);
            occ.insert(b_pos);
            if let Some(path) = path {
                self.meshes[0].links.push(Link {
                    a: honey_id,
                    b,
                    path,
                    drawn: 0,
                    kind: LinkKind::Cross,
                    load: 0,
                    breach_ttl: 0,
                    burn_ticks: 0,
                    quarantined: 0,
                    packets_delivered: 0,
                    is_backbone: false,
            black_market_until: 0,
                    latent: false,
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

    /// When a large subtree is scheduled to cascade, roll for a
    /// "rebirth": one of the doomed nodes stands back up as the root
    /// of a brand-new faction instead of dying, with its own C2
    /// entry, faction slot, persona, and color. Called from
    /// `schedule_subtree_death` where the full cascade cohort is
    /// known, not from advance_dying (where staggered deaths rarely
    /// reach the threshold in a single tick).
    fn maybe_resurrect_c2_from_cascade(&mut self, doomed: &[NodeId]) {
        let threshold = self.cfg.resurrection_threshold as usize;
        if threshold == 0 || doomed.len() < threshold {
            return;
        }
        if self.cfg.resurrection_chance <= 0.0
            || !self.rng.gen_bool(self.cfg.resurrection_chance as f64)
        {
            return;
        }
        // Pick from the doomed pool, skipping anything that's
        // already dead or mid-respawn.
        let mut candidates: Vec<NodeId> = doomed
            .iter()
            .copied()
            .filter(|&id| !matches!(self.meshes[0].nodes[id].state, State::Dead))
            .collect();
        if candidates.is_empty() {
            return;
        }
        let idx = self.rng.gen_range(0..candidates.len());
        let reborn = candidates.swap_remove(idx);
        let new_faction = self.meshes[0].c2_nodes.len() as u8;
        let new_branch = self.alloc_branch_id();
        let pos = self.meshes[0].nodes[reborn].pos;
        // Cancel the pending death and reset the node to a fresh C2
        // state. Old inbound/outbound links stay as ghost wires on
        // the dying subtree around it.
        let node = &mut self.meshes[0].nodes[reborn];
        node.state = State::Alive;
        node.dying_in = 0;
        node.death_echo = 0;
        node.parent = None;
        node.born = self.tick;
        node.role = Role::Relay;
        node.hardened = false;
        node.heartbeats = 0;
        node.pulse = 6;
        node.infection = None;
        node.faction = new_faction;
        node.branch_id = new_branch;
        node.pwn_resist = super::C2_INITIAL_HP;
        node.shield_flash = 0;
        node.mutated_flash = 12;
        node.scan_pulse = 0;
        // Register the new C2 and faction.
        self.meshes[0].c2_nodes.push(reborn);
        self.faction_stats.push(FactionStats::default());
        // Reborn factions roll a fresh persona too. Pick uniformly
        // so the resurrected colony reads as its own player.
        let persona = match self.rng.gen_range(0..4u8) {
            0 => super::Persona::Aggressor,
            1 => super::Persona::Fortress,
            2 => super::Persona::Plague,
            _ => super::Persona::Opportunist,
        };
        self.personas.push(persona);
        // Pick a random palette slot for the reborn colony too so
        // the color swap reads as "new player arrived" instead of
        // inheriting the next sequential hue.
        let palette_len = crate::theme::theme().faction_palette.len().max(1);
        let color_idx = self.rng.gen_range(0..palette_len);
        self.faction_colors.push(color_idx);
        let (a, b) = octet_pair(pos);
        self.push_log(format!(
            "✦ MYTHIC ✦ REBIRTH — c2[{}] rises from the ashes @ 10.0.{}.{}",
            new_faction, a, b
        ));
    }

    pub fn schedule_subtree_death(&mut self, root: NodeId, mult: f32) {
        let cascade = self.compute_cascade(root);
        let mut touched = 0u32;
        let mut doomed: Vec<NodeId> = Vec::with_capacity(cascade.len());
        for (id, distance) in cascade {
            let base = distance.saturating_mul(2).saturating_add(3) as f32;
            let delay = (base * mult).round().clamp(1.0, 255.0) as u8;
            if self.meshes[0].nodes[id].dying_in == 0 || self.meshes[0].nodes[id].dying_in > delay {
                self.meshes[0].nodes[id].dying_in = delay;
                touched += 1;
                doomed.push(id);
            }
        }
        if touched > 0 {
            let label = if mult > 1.5 { "HONEYPOT cascade" } else { "cascade" };
            self.push_log(format!("{}: {} hosts burning", label, touched));
            if (touched as usize) >= self.cfg.mythic_big_one_threshold {
                self.push_log(format!("✦ MYTHIC ✦ THE BIG ONE — {} hosts", touched));
            }
            let root_pos = self.meshes[0].nodes[root].pos;
            self.emit_cascade_effects(root_pos, touched);
            // Roll resurrection at schedule time so the whole
            // cohort is visible instead of tick-by-tick finalizations.
            self.maybe_resurrect_c2_from_cascade(&doomed);
        }
    }

    pub(super) fn advance_dying(&mut self) {
        let mut newly_dead: Vec<NodeId> = Vec::new();
        for (i, n) in self.meshes[0].nodes.iter_mut().enumerate() {
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
        let mut fallen_legends: Vec<(NodeId, (i16, i16), u16, u8)> = Vec::new();
        for id in &newly_dead {
            let faction = self.meshes[0].nodes[*id].faction;
            // Don't double-count: if the node was already in
            // State::Pwned, advance_pwned_and_loss already bumped
            // faction_stats.lost at the exploit moment. Only
            // cascade-only deaths (still Alive when dying_in
            // started) need to be counted here.
            let was_pwned = matches!(self.meshes[0].nodes[*id].state, State::Pwned { .. });
            self.meshes[0].nodes[*id].state = State::Dead;
            self.meshes[0].nodes[*id].death_echo = GHOST_ECHO_TICKS;
            if !was_pwned {
                if let Some(s) = self.faction_stats.get_mut(faction as usize) {
                    s.lost += 1;
                }
            }
            // Legendary nodes become permanent tombstones. Log
            // the fall loudly so the viewer catches the death
            // of a named character.
            if self.meshes[0].nodes[*id].legendary_name != u16::MAX {
                fallen_legends.push((
                    *id,
                    self.meshes[0].nodes[*id].pos,
                    self.meshes[0].nodes[*id].legendary_name,
                    faction,
                ));
            }
        }
        for (_, pos, name_idx, faction) in fallen_legends {
            let pool = super::LEGENDARY_NAME_POOL;
            let name = pool[(name_idx as usize) % pool.len()];
            let (a, b) = octet_pair(pos);
            self.push_log(format!(
                "✦ legend ✦ {} falls @ 10.0.{}.{} (F{})",
                name, a, b, faction
            ));
        }
        // Faction memory decay: when a C2 node dies, purge every
        // relation entry involving that faction from the unified
        // diplomacy map. The faction is gone — so are its
        // grudges, alliances, NAPs, trades, and vassalage claims.
        // Reborn colonies start with a clean slate, and surviving
        // rivals stop escalating against a faction that no
        // longer exists.
        let dead_factions: Vec<u8> = newly_dead
            .iter()
            .filter(|&&id| self.meshes[0].c2_nodes.contains(&id))
            .map(|&id| self.meshes[0].nodes[id].faction)
            .collect();
        if !dead_factions.is_empty() {
            let before = self.relations.len();
            self.relations
                .retain(|&(a, b), _| !dead_factions.iter().any(|&f| a == f || b == f));
            let purged = before.saturating_sub(self.relations.len());
            for f in &dead_factions {
                self.push_log(format!(
                    "F{} memory fades — {} relations forgotten",
                    f, purged
                ));
            }
            // Release any strain patents held by the dead
            // factions — dead factions don't collect royalties.
            for slot in self.strain_patents.iter_mut() {
                if let Some(owner) = *slot {
                    if dead_factions.contains(&owner) {
                        *slot = None;
                    }
                }
            }
        }
        // Resurrection rolls now happen at schedule_subtree_death
        // time, where the whole cascade cohort is visible — by the
        // point a node finalizes into Dead its cohort has long
        // since been spread across multiple ticks.
        // Free cells of links that now touch a Dead endpoint so territory reopens.
        let dead: HashSet<NodeId> = self
            .meshes[0]
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
        // Split self.meshes[0] into one mutable borrow so the
        // iterations over its fields don't fight the subsequent
        // mutation of occupied.
        let mesh = &mut self.meshes[0];
        let c2_positions: HashSet<(i16, i16)> =
            mesh.c2_nodes.iter().map(|&id| mesh.nodes[id].pos).collect();
        // Collect cells to remove before mutating occupied so
        // the immutable link iter doesn't alias the mutable
        // occupied write.
        let mut cells_to_remove: Vec<(i16, i16)> = Vec::new();
        for link in &mesh.links {
            if dead.contains(&link.a) || dead.contains(&link.b) {
                for c in &link.path {
                    if !c2_positions.contains(c) {
                        cells_to_remove.push(*c);
                    }
                }
            }
        }
        for c in cells_to_remove {
            mesh.occupied.remove(&c);
        }
        // Re-seat alive node cells in case we just removed one.
        let alive_positions: Vec<(i16, i16)> = mesh
            .nodes
            .iter()
            .filter(|n| !matches!(n.state, State::Dead))
            .map(|n| n.pos)
            .collect();
        for pos in alive_positions {
            mesh.occupied.insert(pos);
        }
        // Ghost cleanup: non-legendary dead nodes whose death_echo
        // has fully expired release their cell from `occupied`
        // so new spawns and routing can reclaim the space.
        // Legendary dead nodes keep their cell held as permanent
        // tombstones — the cell stays occupied and the render
        // pass keeps drawing a tombstone glyph there.
        let ghost_cells: Vec<(i16, i16)> = mesh
            .nodes
            .iter()
            .filter(|n| {
                matches!(n.state, State::Dead)
                    && n.death_echo == 0
                    && n.legendary_name == u16::MAX
            })
            .map(|n| n.pos)
            .collect();
        for pos in ghost_cells {
            mesh.occupied.remove(&pos);
        }
    }
}
