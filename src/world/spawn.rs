//! Node spawning, branch allocation, link rewiring, and parent
//! selection.
//!
//! Covers the growth side of the state machine: `try_spawn` picks a
//! parent and places a new node, `maybe_reconnect` drops lateral
//! cross-links between live branches, and the smaller helpers
//! (`roll_role`, `alloc_branch_id`, `build_inbound_links`, `depth_of`)
//! back them up. Split out of `world/mod.rs` so the core tick loop
//! does not have to carry the roughly 400 lines of spawning logic.

use std::collections::{HashMap, HashSet};

use rand::Rng;

use crate::routing;

use super::{Link, LinkKind, Node, NodeId, Role, State, World, DIRS};

impl World {
    pub(super) fn maybe_reconnect(&mut self) {
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
        // Rivalry-amplified: a faction's max pressure against any
        // other faction adds up to 2x to the base cross-faction
        // chance, so live feuds pull more bridges between rivals.
        let max_pressure = (0..self.faction_stats.len() as u8)
            .filter(|&f| f != a_faction)
            .map(|f| self.rivalry_pressure(a_faction, f))
            .max()
            .unwrap_or(0);
        let amp = 1.0 + (max_pressure as f32 / super::RIVALRY_CAP as f32);
        let tech_bridge = self.tech_effects(a_faction).bridge_mult;
        let cross_chance = (self.cfg.cross_faction_bridge_chance
            * amp
            * self.era_rules.bridge_mult
            * tech_bridge)
            .min(1.0) as f64;
        let allow_cross_faction =
            self.cfg.cross_faction_bridge_chance > 0.0 && self.rng.gen_bool(cross_chance);
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
        // Routing only blocks on node cells, not existing link
        // paths — so new links are free to cross over already-
        // drawn wires and produce denser mesh topology.
        let node_cells: std::collections::HashSet<(i16, i16)> =
            self.nodes.iter().map(|n| n.pos).collect();
        let path = match routing::route_link(
            a_pos,
            b_pos,
            &node_cells,
            self.bounds,
            &mut self.rng,
        ) {
            Some(p) => p,
            None => return,
        };
        // Sleeper lattice: some reconnect links start latent —
        // dormant and invisible until a trigger (owner at war,
        // or one endpoint isolated from its parent chain)
        // activates them. Roughly one in four new cross-links
        // are sleepers.
        let latent = self.nodes[a].faction == self.nodes[b].faction
            && self.rng.gen_bool(0.25);
        self.links.push(Link {
            a,
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
            latent,
        });
        if latent {
            // Don't announce sleeper edges — they're hidden
            // until activation. Skip the bridge log entirely.
            return;
        }
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

    /// Roll a role for a new node belonging to `faction`. The
    /// faction's Persona biases the base role weights so different
    /// factions feel like distinct players. Falls back to the raw
    /// cfg weights for Opportunist or when no persona is set.
    fn roll_role(&mut self, faction: u8) -> Role {
        let base = &self.cfg.role_weights;
        let persona = self
            .personas
            .get(faction as usize)
            .copied()
            .unwrap_or(super::Persona::Opportunist);
        // Per-persona multipliers applied to base weights. Each
        // persona shifts emphasis without ever zeroing a role out
        // so the role pool stays diverse for every faction.
        // Multipliers now include a Hunter slot. Fortress favors
        // hunters heavily (defensive culling), Aggressor lightly,
        // Plague suppresses them.
        let (m_relay, m_scan, m_exfil, m_def, m_tow, m_bea, m_prox, m_router, m_hunter) =
            match persona {
                super::Persona::Aggressor => {
                    (1.0, 1.8, 1.6, 0.5, 0.5, 0.7, 1.2, 0.8, 1.0)
                }
                super::Persona::Fortress => {
                    (0.9, 0.7, 0.5, 1.8, 2.0, 1.6, 0.7, 1.4, 1.8)
                }
                super::Persona::Plague => {
                    (1.0, 1.0, 1.4, 0.6, 0.6, 0.9, 1.6, 1.0, 0.4)
                }
                super::Persona::Opportunist => {
                    (1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0)
                }
            };
        // Tier 1+ tech amplifies the persona's deviation from the
        // baseline `1.0` multiplier. A Fortress at Tier 2 with
        // intensity `1.6` pulls its 2.0× tower bias out to
        // `1.0 + (2.0 - 1.0) * 1.6 = 2.6`. Opportunist's all-1.0
        // weights are untouched because there's no deviation to
        // amplify.
        let intensity = self.tech_effects(faction).role_intensity;
        let amp = |m: f32| 1.0 + (m - 1.0) * intensity;
        let m_relay = amp(m_relay);
        let m_scan = amp(m_scan);
        let m_exfil = amp(m_exfil);
        let m_def = amp(m_def);
        let m_tow = amp(m_tow);
        let m_bea = amp(m_bea);
        let m_prox = amp(m_prox);
        let m_router = amp(m_router);
        let m_hunter = amp(m_hunter);
        let w_relay = base.relay * m_relay;
        let w_scanner = base.scanner * m_scan;
        let w_exfil = base.exfil * m_exfil;
        let w_honeypot = base.honeypot;
        let w_defender = base.defender * m_def;
        let w_tower = base.tower * m_tow;
        let w_beacon = base.beacon * m_bea;
        let w_proxy = base.proxy * m_prox;
        let w_decoy = base.decoy;
        let w_router = base.router * m_router;
        let w_hunter = base.hunter * m_hunter;
        let total = w_relay
            + w_scanner
            + w_exfil
            + w_honeypot
            + w_defender
            + w_tower
            + w_beacon
            + w_proxy
            + w_decoy
            + w_router
            + w_hunter;
        let mut r = self.rng.gen::<f32>() * total.max(f32::EPSILON);
        if r < w_relay {
            return Role::Relay;
        }
        r -= w_relay;
        if r < w_scanner {
            return Role::Scanner;
        }
        r -= w_scanner;
        if r < w_exfil {
            return Role::Exfil;
        }
        r -= w_exfil;
        if r < w_honeypot {
            return Role::Honeypot;
        }
        r -= w_honeypot;
        if r < w_defender {
            return Role::Defender;
        }
        r -= w_defender;
        if r < w_tower {
            return Role::Tower;
        }
        r -= w_tower;
        if r < w_beacon {
            return Role::Beacon;
        }
        r -= w_beacon;
        if r < w_proxy {
            return Role::Proxy;
        }
        r -= w_proxy;
        if r < w_decoy {
            return Role::Decoy;
        }
        r -= w_decoy;
        if r < w_router {
            return Role::Router;
        }
        Role::Hunter
    }

    pub(super) fn alloc_branch_id(&mut self) -> u16 {
        let id = self.next_branch_id;
        self.next_branch_id = self.next_branch_id.wrapping_add(1).max(1);
        id
    }

    /// Map each node to the index of its inbound parent link, if any.
    /// Cross-links are deliberately skipped — packets ride parent chains
    /// only, and cascade reachability has its own adjacency builder.
    pub(super) fn build_inbound_links(&self) -> HashMap<NodeId, usize> {
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

    pub(super) fn try_spawn(&mut self) {
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
        let effective_spawn =
            (self.cfg.p_spawn * spawn_mult * self.era_rules.spawn_mult).clamp(0.0, 1.0);
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
                    // Faction favoritism boost: when the user has
                    // engaged a 1-9 hotkey, multiply that faction's
                    // candidate weight so it dominates the parent
                    // roll for the favor window.
                    if self.is_favored(n.faction) {
                        weight *= super::FAVOR_WEIGHT_MULT;
                    }
                    // Turf graffiti: any candidate near a live
                    // mark gets a per-mark multiplicative bump
                    // toward being picked as the next spawn parent.
                    weight *= self.graffiti_weight_bonus(n.pos);
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
        // ISP outages block any new spawn whose target cell falls
        // inside a dead zone — the region is offline so the mesh
        // can't route there.
        if self.outages.iter().any(|o| o.contains(cand)) {
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

        // Only other node positions block the router; link paths
        // are free to cross over existing wires.
        let node_cells: std::collections::HashSet<(i16, i16)> =
            self.nodes.iter().map(|n| n.pos).collect();
        let path = match routing::route_link(
            parent_pos,
            cand,
            &node_cells,
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
        let faction = self.nodes[parent_id].faction;
        let mut role = self.roll_role(faction);

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
        // Terrain bonus: nodes spawned inside a fiber zone start
        // with a small pwn_resist boost, giving factions that
        // expand into a hotspot a defensive head-start.
        if self.hotspots.iter().any(|h| h.contains(cand)) {
            node.pwn_resist = node.pwn_resist.saturating_add(3);
            node.mutated_flash = 4;
        }
        // Sleeper agent: rare chance the node is secretly loyal to a
        // different faction. Only viable when at least two factions
        // exist and the role isn't a stealth/specialist that already
        // has its own deception (Honeypot/Decoy/Defender/Tower/Beacon).
        let faction_count = self.faction_stats.len() as u8;
        let role_eligible = matches!(role, Role::Relay | Role::Scanner | Role::Exfil | Role::Proxy);
        if faction_count >= 2
            && role_eligible
            && self.cfg.sleeper_spawn_chance > 0.0
            && self.rng.gen_bool(self.cfg.sleeper_spawn_chance as f64)
        {
            // Pick any other faction uniformly.
            let mut true_f = self.rng.gen_range(0..faction_count);
            if true_f == faction {
                true_f = (true_f + 1) % faction_count;
            }
            node.sleeper_true_faction = Some(true_f);
        }
        self.nodes.push(node);
        if let Some(s) = self.faction_stats.get_mut(faction as usize) {
            s.spawned += 1;
        }
        // Credit the parent so legendary-node promotion can track
        // reproductive success alongside raw age.
        self.nodes[parent_id].children_spawned =
            self.nodes[parent_id].children_spawned.saturating_add(1);
        self.occupied.insert(cand);
        self.links.push(Link {
            a: parent_id,
            b: new_id,
            path,
            drawn: 0,
            kind: LinkKind::Parent,
            load: 0,
            breach_ttl: 0,
            burn_ticks: 0,
            quarantined: 0,
            packets_delivered: 0,
            is_backbone: false,
            latent: false,
        });

        let h = (cand.0 as u32).wrapping_mul(2654435761) ^ (cand.1 as u32).wrapping_mul(40503);
        let a = (h >> 16) & 0xff;
        let b = (h >> 8) & 0xff;
        let c = h & 0xff;
        self.push_log(format!("handshake 10.{}.{}.{} OK", a, b, c));
    }

    pub(super) fn depth_of(&self, mut id: NodeId) -> u32 {
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
}
