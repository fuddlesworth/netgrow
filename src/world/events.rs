//! Periodic and environmental events: alliances, border skirmishes,
//! assimilation, wormholes, DDoS waves, and network storms.
//!
//! Each entry point is a `maybe_*` or `advance_*` method the tick loop
//! calls on a fixed cadence. Split out of `world/mod.rs` so the core
//! state machine does not have to inline every flavor subsystem.

use rand::Rng;

use super::{Alliance, DdosWave, IspOutage, NodeId, State, World, Wormhole, octet_pair};

impl World {
    pub(super) fn maybe_alliance(&mut self) {
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
    pub(super) fn maybe_border_skirmish(&mut self) {
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
            let pos = self.nodes[id].pos;
            let victim_faction = self.nodes[id].faction;
            let hardened = self.nodes[id].hardened;
            if hardened {
                let node = &mut self.nodes[id];
                node.hardened = false;
                node.heartbeats = 0;
                node.shield_flash = 6;
                self.log_node(pos, "skirmish shielded");
                self.bump_rivalry(victim_faction, enemy_faction, 2);
            } else {
                let node = &mut self.nodes[id];
                node.state = State::Pwned {
                    ticks_left: pwned_flash,
                };
                self.log_node(pos, "skirmish LOST");
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
        self.nodes[weak_c2].death_echo = super::GHOST_ECHO_TICKS;

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

    pub(super) fn maybe_wormhole(&mut self) {
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

    pub(super) fn advance_wormholes(&mut self) {
        for wh in self.wormholes.iter_mut() {
            wh.age = wh.age.saturating_add(1);
        }
        self.wormholes.retain(|w| w.age < w.life);
    }

    /// Roll for a new ISP outage zone. Picks a random rectangle on
    /// the mesh and registers it; spawns are blocked and alive
    /// nodes inside take a steady stun until the outage dissolves.
    pub(super) fn maybe_isp_outage(&mut self) {
        if !self.roll_periodic(self.cfg.isp_outage_period, self.cfg.isp_outage_chance) {
            return;
        }
        let bounds = self.bounds;
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
        self.outages.push(outage);
        self.push_log("⚠ ISP OUTAGE — region offline".to_string());
    }

    pub(super) fn advance_outages(&mut self) {
        if self.outages.is_empty() {
            return;
        }
        // Snapshot active rectangles for the per-node stun pass
        // without aliasing self.outages.
        let zones: Vec<(i16, i16, i16, i16)> = self
            .outages
            .iter()
            .map(|o| (o.min.0, o.min.1, o.max.0, o.max.1))
            .collect();
        for n in self.nodes.iter_mut() {
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
        for o in self.outages.iter_mut() {
            o.age = o.age.saturating_add(1);
        }
        let dissolved = self.outages.iter().filter(|o| o.age >= o.life).count();
        self.outages.retain(|o| o.age < o.life);
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

    pub(super) fn advance_ddos_waves(&mut self) {
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
    pub(super) fn maybe_storm(&mut self) {
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
        self.storm_since = self.tick;
        // Storm front always rolls downward from the top edge and
        // can drift left, straight, or right. Picked fresh for each
        // storm so consecutive storms read as distinct weather.
        let dx = self.rng.gen_range(-1..=1i8);
        self.storm_dir = (dx, 1);
        self.push_log("⚡ STORM — mesh destabilizing".to_string());
    }
}
