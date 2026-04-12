//! Periodic and environmental events: alliances, border skirmishes,
//! assimilation, wormholes, DDoS waves, and network storms.
//!
//! Each entry point is a `maybe_*` or `advance_*` method the tick loop
//! calls on a fixed cadence. Split out of `world/mod.rs` so the core
//! state machine does not have to inline every flavor subsystem.

use rand::Rng;

use super::{DdosWave, IspOutage, NodeId, Partition, State, World, Wormhole, octet_pair};

impl World {
    /// Border skirmishes: periodic low-probability hits on nodes that
    /// sit near an enemy-faction neighbor. Visible as scattered
    /// shielded/LOST lines at faction frontiers during long runs.
    pub(super) fn maybe_border_skirmish(&mut self) {
        if !self.roll_periodic(self.cfg.border_skirmish_period, 1.0) {
            return;
        }
        if self.meshes[0].c2_nodes.len() < 2 {
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
            .meshes[0]
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
        // (victim_id, enemy_faction). The enemy is whichever
        // different-faction neighbor sits closest within the radius —
        // used to attribute the kill for rivalry bookkeeping.
        let mut victims: Vec<(NodeId, u8)> = Vec::new();
        for &(id, pos, faction) in &positions {
            let mut nearest_enemy: Option<(i16, u8)> = None;
            for &(_, p, f) in &positions {
                if f == faction || self.allied(f, faction) {
                    continue;
                }
                let d = (p.0 - pos.0).abs().max((p.1 - pos.1).abs());
                if d <= radius && nearest_enemy.map(|(nd, _)| d < nd).unwrap_or(true) {
                    nearest_enemy = Some((d, f));
                }
            }
            let Some((_, enemy_faction)) = nearest_enemy else {
                continue;
            };
            // Rivalry-amplified chance: an old feud's pressure adds
            // up to a 2x multiplier on the base skirmish_chance.
            // An active open war declaration stacks an additional
            // flat 3x on top so declared wars feel hotter than
            // background feuds.
            let pressure = self.rivalry_pressure(faction, enemy_faction);
            let mut amp = 1.0 + (pressure as f32 / super::RIVALRY_CAP as f32);
            if self.at_war(faction, enemy_faction) {
                amp *= 3.0;
            }
            let effective = (chance * amp).min(1.0);
            if self.rng.gen_bool(effective as f64) {
                victims.push((id, enemy_faction));
            }
        }
        for (id, enemy_faction) in victims {
            let pos = self.meshes[0].nodes[id].pos;
            let victim_faction = self.meshes[0].nodes[id].faction;
            let hardened = self.meshes[0].nodes[id].hardened;
            if hardened {
                let node = &mut self.meshes[0].nodes[id];
                node.hardened = false;
                node.heartbeats = 0;
                node.shield_flash = 6;
                self.log_node(pos, "skirmish shielded");
                self.bump_rivalry(victim_faction, enemy_faction, 2);
            } else {
                let node = &mut self.meshes[0].nodes[id];
                node.state = State::Pwned {
                    ticks_left: pwned_flash,
                };
                self.log_node(
                    pos,
                    &format!("skirmish LOST F{}→F{}", enemy_faction, victim_faction),
                );
                self.bump_rivalry(victim_faction, enemy_faction, 6);
            }
        }
    }

    /// Faction extinction mechanic. When a faction drops below
    /// assimilation_threshold alive nodes and another faction has at
    /// least assimilation_dominance alive nodes, the weak faction's
    /// remaining nodes flip to the strongest faction's color and its
    /// C2 is marked dead — visible as a dramatic color swap + mythic
    /// log line.
    pub(super) fn maybe_assimilate(&mut self) {
        let period = {
            let speed = self.era_rules.assimilation_speed_mult.max(0.01);
            let scaled = (self.cfg.assimilation_period as f32 / speed) as u64;
            scaled.max(1)
        };
        if !self.roll_periodic(period, 1.0) {
            return;
        }
        if self.meshes[0].c2_nodes.len() < 2 {
            return;
        }
        // Count alive per faction.
        let mut counts = vec![0usize; self.faction_stats.len()];
        for n in &self.meshes[0].nodes {
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
        let weak_c2 = self.meshes[0].c2_nodes[weak_idx];
        self.meshes[0].nodes[weak_c2].state = State::Dead;
        self.meshes[0].nodes[weak_c2].death_echo = super::GHOST_ECHO_TICKS;

        // Snapshot strong-faction alive positions up front so we can
        // reparent each absorbed node to its nearest strong neighbor.
        // Without this step the flipped nodes keep their old parent
        // (the now-dead weak C2), which immediately isolates them
        // from the strong C2's reachability tree and dooms them.
        let strong_positions: Vec<(NodeId, (i16, i16))> = self
            .meshes[0]
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
            .meshes[0]
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
            let pos = self.meshes[0].nodes[id].pos;
            // Nearest strong-faction alive node becomes the new parent.
            // Fall back to the strong C2 if no other strong nodes exist.
            let new_parent = strong_positions
                .iter()
                .min_by_key(|(_, p)| (p.0 - pos.0).abs().max((p.1 - pos.1).abs()))
                .map(|(pid, _)| *pid)
                .unwrap_or(self.meshes[0].c2_nodes[strong_idx]);
            self.meshes[0].nodes[id].faction = new_faction;
            self.meshes[0].nodes[id].parent = Some(new_parent);
            flipped += 1;
        }

        self.push_log(format!(
            "✦ MYTHIC ✦ F{} absorbed by F{} ({} hosts)",
            weak_idx, strong_idx, flipped
        ));
    }

    /// Roll for a defector event. Picks a random non-C2 alive
    /// node, flips its `faction` to a random rival, reparents it
    /// to the nearest alive node of the new faction (so it
    /// doesn't get immediately cascaded by losing its parent
    /// chain), and credits the receiving faction with
    /// `defector_intel_reward` intel as "topology memory carried
    /// across the lines." Logs a mythic defector line with the
    /// old → new faction arrow and the defector's IP.
    pub(super) fn maybe_defector(&mut self) {
        if !self.roll_periodic(self.cfg.defector_period, self.cfg.defector_chance) {
            return;
        }
        if self.meshes[0].c2_nodes.len() < 2 {
            return;
        }
        // Candidate defectors: alive, non-C2, and the defector's
        // faction must have at least one **other** faction alive
        // for the pool to make sense.
        let candidates: Vec<NodeId> = self
            .meshes[0]
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                if !matches!(n.state, State::Alive) || self.is_c2(i) {
                    return None;
                }
                Some(i)
            })
            .collect();
        if candidates.is_empty() {
            return;
        }
        let defector = candidates[self.rng.gen_range(0..candidates.len())];
        let old_faction = self.meshes[0].nodes[defector].faction;
        let old_pos = self.meshes[0].nodes[defector].pos;
        // Rival factions: any faction id other than the defector's
        // current one whose C2 is still Alive.
        let rivals: Vec<u8> = self
            .meshes[0]
            .c2_nodes
            .iter()
            .enumerate()
            .filter_map(|(i, &cid)| {
                if i as u8 == old_faction {
                    return None;
                }
                matches!(self.meshes[0].nodes[cid].state, State::Alive).then_some(i as u8)
            })
            .collect();
        if rivals.is_empty() {
            return;
        }
        let new_faction = rivals[self.rng.gen_range(0..rivals.len())];
        // Find the nearest alive node of the new faction — that's
        // the reparent anchor. Fall back to the new faction's C2.
        let anchor = self
            .meshes[0]
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                if n.faction == new_faction && matches!(n.state, State::Alive) && i != defector
                {
                    let d = (n.pos.0 - old_pos.0).abs().max((n.pos.1 - old_pos.1).abs());
                    Some((i, d))
                } else {
                    None
                }
            })
            .min_by_key(|(_, d)| *d)
            .map(|(i, _)| i)
            .unwrap_or(self.meshes[0].c2_nodes[new_faction as usize]);
        // Flip the defector.
        {
            let n = &mut self.meshes[0].nodes[defector];
            n.faction = new_faction;
            n.parent = Some(anchor);
            n.mutated_flash = 12;
        }
        // Credit the receiving faction with topology memory.
        let reward = self.cfg.defector_intel_reward;
        if let Some(s) = self.faction_stats.get_mut(new_faction as usize) {
            s.intel = s.intel.saturating_add(reward);
        }
        let (oa, ob) = octet_pair(old_pos);
        self.push_log(format!(
            "✦ MYTHIC ✦ F{} → F{} defector @ 10.0.{}.{} (+{} intel)",
            old_faction, new_faction, oa, ob, reward
        ));
    }

    pub(super) fn maybe_wormhole(&mut self) {
        if !self.roll_periodic(self.cfg.wormhole_period, self.cfg.wormhole_chance) {
            return;
        }
        // Pick two distinct alive nodes to link.
        let alive: Vec<NodeId> = self
            .meshes[0]
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
        let a_pos = self.meshes[0].nodes[a].pos;
        let b_pos = self.meshes[0].nodes[b].pos;
        let life = self.cfg.wormhole_life_ticks;
        self.meshes[0].wormholes.push(Wormhole {
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

    pub(super) fn advance_wormholes(&mut self) {
        for wh in self.meshes[0].wormholes.iter_mut() {
            wh.age = wh.age.saturating_add(1);
        }
        self.meshes[0].wormholes.retain(|w| w.age < w.life);
    }

    /// Roll for a new ISP outage zone. Picks a random rectangle on
    /// the mesh and registers it; spawns are blocked and alive
    /// nodes inside take a steady stun until the outage dissolves.
    pub(super) fn maybe_isp_outage(&mut self) {
        if !self.roll_periodic(self.cfg.isp_outage_period, self.cfg.isp_outage_chance) {
            return;
        }
        let bounds = self.meshes[0].bounds;
        if bounds.0 < 4 || bounds.1 < 4 {
            return;
        }
        let min_side = self.cfg.isp_outage_min_side.max(2);
        let max_side = self.cfg.isp_outage_max_side.max(min_side);
        let w = self.rng.gen_range(min_side..=max_side).min(bounds.0 - 2);
        let h = self.rng.gen_range(min_side..=max_side).min(bounds.1 - 2);
        let x0 = self.rng.gen_range(0..(bounds.0 - w));
        let y0 = self.rng.gen_range(0..(bounds.1 - h));
        let outage = IspOutage {
            min: (x0, y0),
            max: (x0 + w, y0 + h),
            age: 0,
            life: self.cfg.isp_outage_life_ticks,
        };
        self.meshes[0].outages.push(outage);
        self.push_log("⚠ ISP OUTAGE — region offline".to_string());
    }

    /// Roll for a new network partition. Picks a random orientation
    /// (horizontal or vertical) and a position somewhere in the
    /// middle third of the mesh so the cut always separates a
    /// meaningful portion of the network.
    pub(super) fn maybe_partition(&mut self) {
        if !self.roll_periodic(self.cfg.partition_period, self.cfg.partition_chance) {
            return;
        }
        let bounds = self.meshes[0].bounds;
        if bounds.0 < 6 || bounds.1 < 6 {
            return;
        }
        let horizontal = self.rng.gen_bool(0.5);
        let pos = if horizontal {
            self.rng.gen_range(bounds.1 / 3..(bounds.1 * 2 / 3))
        } else {
            self.rng.gen_range(bounds.0 / 3..(bounds.0 * 2 / 3))
        };
        self.meshes[0].partitions.push(Partition {
            horizontal,
            pos,
            age: 0,
            life: self.cfg.partition_life_ticks,
        });
        let axis = if horizontal { "horizontal" } else { "vertical" };
        self.push_log(format!("✂ PARTITION — {} cut at {}", axis, pos));
    }

    pub(super) fn advance_partitions(&mut self) {
        if self.meshes[0].partitions.is_empty() {
            return;
        }
        for p in self.meshes[0].partitions.iter_mut() {
            p.age = p.age.saturating_add(1);
        }
        let dissolved = self.meshes[0].partitions.iter().filter(|p| p.age >= p.life).count();
        self.meshes[0].partitions.retain(|p| p.age < p.life);
        if dissolved > 0 {
            self.push_log("partition healed".to_string());
        }
    }

    pub(super) fn advance_outages(&mut self) {
        if self.meshes[0].outages.is_empty() {
            return;
        }
        // Snapshot active rectangles for the per-node stun pass
        // without aliasing self.meshes[0].outages.
        let zones: Vec<(i16, i16, i16, i16)> = self
            .meshes[0]
            .outages
            .iter()
            .map(|o| (o.min.0, o.min.1, o.max.0, o.max.1))
            .collect();
        for n in self.meshes[0].nodes.iter_mut() {
            if !matches!(n.state, State::Alive) {
                continue;
            }
            for &(x0, y0, x1, y1) in &zones {
                if n.pos.0 >= x0 && n.pos.0 <= x1 && n.pos.1 >= y0 && n.pos.1 <= y1 {
                    // Mild rolling stun while inside the dead zone.
                    n.role_cooldown = n.role_cooldown.saturating_add(4).min(500);
                    break;
                }
            }
        }
        // Black-market links collapse when an ISP outage touches
        // any cell in their path. Unlicensed fiber is
        // structurally fragile — ISP pressure instantly kills
        // the uplift. Cleared by setting black_market_until back
        // to 0 so effective_hot_link reverts to baseline.
        let mut collapsed = 0u32;
        for link in self.meshes[0].links.iter_mut() {
            if link.black_market_until <= self.tick {
                continue;
            }
            for &(x0, y0, x1, y1) in &zones {
                if link.path.iter().any(|&(px, py)| {
                    px >= x0 && px <= x1 && py >= y0 && py <= y1
                }) {
                    link.black_market_until = 0;
                    link.load = 0;
                    collapsed += 1;
                    break;
                }
            }
        }
        if collapsed > 0 {
            self.push_log(format!(
                "black market collapse — {} uplinks seized",
                collapsed
            ));
        }
        for o in self.meshes[0].outages.iter_mut() {
            o.age = o.age.saturating_add(1);
        }
        let dissolved = self.meshes[0].outages.iter().filter(|o| o.age >= o.life).count();
        self.meshes[0].outages.retain(|o| o.age < o.life);
        if dissolved > 0 {
            self.push_log("ISP outage cleared".to_string());
        }
    }

    pub(super) fn maybe_ddos(&mut self) {
        if !self.roll_periodic(self.cfg.ddos_period, self.cfg.ddos_chance) {
            return;
        }
        // Pick a random edge to originate from and sweep toward the
        // opposite side.
        let edge = self.rng.gen_range(0..4u8);
        let (horizontal, pos, direction) = match edge {
            0 => (true, 0, 1),                    // top, moving down
            1 => (true, self.meshes[0].bounds.1 - 1, -1),   // bottom, moving up
            2 => (false, 0, 1),                   // left, moving right
            _ => (false, self.meshes[0].bounds.0 - 1, -1),  // right, moving left
        };
        self.meshes[0].ddos_waves.push(DdosWave {
            pos,
            horizontal,
            direction,
            age: 0,
        });
        self.push_log("⚡ DDOS wave inbound".to_string());
    }

    pub(super) fn advance_ddos_waves(&mut self) {
        if self.meshes[0].ddos_waves.is_empty() {
            return;
        }
        let stun = self.cfg.ddos_stun_ticks;
        let bounds = self.meshes[0].bounds;
        let mut keep: Vec<DdosWave> = Vec::with_capacity(self.meshes[0].ddos_waves.len());
        for mut wave in std::mem::take(&mut self.meshes[0].ddos_waves) {
            // Apply stun to any alive node whose position coincides
            // with the current wave line.
            for n in self.meshes[0].nodes.iter_mut() {
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
        self.meshes[0].ddos_waves = keep;
    }

    /// Roll for a network storm. Storms spike both spawn and loss rates
    /// for a configurable duration and log the start / end transitions.
    pub(super) fn maybe_storm(&mut self) {
        // End an active storm when its window elapses.
        if self.meshes[0].storm_until > 0 && self.tick >= self.meshes[0].storm_until {
            self.meshes[0].storm_until = 0;
            self.push_log("storm passes — mesh settling".to_string());
            return;
        }
        // Roll for a new storm only when one isn't already active.
        if self.meshes[0].storm_until > 0 {
            return;
        }
        if !self.roll_periodic(self.cfg.storm_period, self.cfg.storm_chance) {
            return;
        }
        self.meshes[0].storm_until = self.tick + self.cfg.storm_duration;
        self.meshes[0].storm_since = self.tick;
        // Storm front always rolls downward from the top edge and
        // can drift left, straight, or right. Picked fresh for each
        // storm so consecutive storms read as distinct weather.
        let dx = self.rng.gen_range(-1..=1i8);
        self.meshes[0].storm_dir = (dx, 1);
        self.push_log("⚡ STORM — mesh destabilizing".to_string());
    }
}
