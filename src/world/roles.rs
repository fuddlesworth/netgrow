//! Per-role tick behaviors: scanner pings, exfil packet emission,
//! defender pulses, and role-cooldown bookkeeping.
//!
//! Split out of `world/mod.rs` so the role behaviors can live in one
//! place. `advance_role_cooldowns` decays the transient timers that
//! gate every firing; the `fire_*` methods drive the actual role-
//! specific effects the tick loop triggers each step.

use rand::Rng;

use super::{
    NodeId, Packet, Role, State, Worm, World, DIRS, HOT_LINK, SCANNER_PULSE_TICKS, WARM_LINK,
};

impl World {
    pub(super) fn advance_role_cooldowns(&mut self) {
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
            if n.death_echo > 0 {
                n.death_echo -= 1;
            }
            if n.immunity_ticks > 0 {
                n.immunity_ticks -= 1;
                if n.immunity_ticks == 0 {
                    n.immunity_strain = None;
                }
            }
        }
    }

    pub(super) fn fire_scanner_pings(&mut self) {
        let base_period = self.cfg.scanner_ping_period;
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
        // (scanner_pos, scanner_faction) — collected so the
        // sighting pass downstream can attribute each spot to the
        // right faction without re-reading the scanner's state.
        let mut fired_positions: Vec<(i16, i16)> = Vec::new();
        let mut scanner_info: Vec<((i16, i16), u8)> = Vec::new();
        for id in scanner_ids {
            // Pick a direction so the spawn bias in try_spawn still favors
            // growth along the scanner's last sweep. Pulled out of the
            // mut borrow below to avoid aliasing.
            let dir_idx = self.rng.gen_range(0..DIRS.len());
            let (dx, dy) = DIRS[dir_idx];
            // Synergy: a scanner adjacent to a Beacon doubles its
            // pulse duration so the linked-up cluster reads as a
            // brighter scan zone for twice as long.
            let pos = self.nodes[id].pos;
            let beacon_boost = self.has_neighbor_role(pos, Role::Beacon);
            let pulse_ticks = if beacon_boost {
                SCANNER_PULSE_TICKS.saturating_mul(2)
            } else {
                SCANNER_PULSE_TICKS
            };
            let faction = self.nodes[id].faction;
            // Aggressor Tier 2 scales scanner period down, so
            // recon fires more often for tech-advanced factions.
            let period = ((base_period as f32)
                * self.tech_effects(faction).scanner_period_mult)
            .max(1.0) as u16;
            let n = &mut self.nodes[id];
            n.role_cooldown = period;
            n.last_ping_tick = now;
            n.last_ping_dir = Some((dx as i8, dy as i8));
            n.scan_pulse = pulse_ticks;
            fired_positions.push(n.pos);
            scanner_info.push((n.pos, faction));
        }
        // Scanner sightings: each fired scanner rolls a small
        // chance to spot one enemy-faction alive node within its
        // pulse radius and log it. Rate-limited to one sighting
        // per scanner per fire so the log doesn't flood.
        if !scanner_info.is_empty() {
            let sight_radius = self.cfg.proxy_radius;
            for (spos, sfaction) in &scanner_info {
                // Collect candidate enemy positions for this
                // scanner and pick one.
                let mut cands: Vec<(i16, i16, u8)> = Vec::new();
                for n in &self.nodes {
                    if !matches!(n.state, State::Alive) {
                        continue;
                    }
                    if n.faction == *sfaction {
                        continue;
                    }
                    let d = (n.pos.0 - spos.0).abs().max((n.pos.1 - spos.1).abs());
                    if d <= sight_radius {
                        cands.push((n.pos.0, n.pos.1, n.faction));
                    }
                }
                if cands.is_empty() {
                    continue;
                }
                if !self.rng.gen_bool(0.35) {
                    continue;
                }
                let (ex, ey, ef) = cands[self.rng.gen_range(0..cands.len())];
                let (oa, ob) = super::octet_pair((ex, ey));
                self.push_log(format!(
                    "F{} scanner spotted F{} asset @ 10.0.{}.{}",
                    sfaction, ef, oa, ob
                ));
            }
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

    pub(super) fn fire_exfil_packets(&mut self) {
        let period = ((self.cfg.exfil_packet_period as f32)
            * self.era_rules.exfil_period_mult)
            .max(1.0) as u16;
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
            if let Some(&link_id) = inbound.get(&id) {
                let link = &self.links[link_id];
                if link.path.is_empty() {
                    self.nodes[id].role_cooldown = period;
                    continue;
                }
                // Synergy: an exfil adjacent to a Router gets a much
                // higher backpressure ceiling — its packets can ride
                // up to HOT_LINK before throttling instead of WARM.
                // Rewards spawning routers next to busy exfils.
                let exfil_pos = self.nodes[id].pos;
                let router_adjacent =
                    self.has_neighbor_role(exfil_pos, Role::Router);
                let pressure_ceiling = if router_adjacent { HOT_LINK } else { WARM_LINK };
                if link.load >= pressure_ceiling || link.quarantined > 0 {
                    // Quick-retry backpressure: shorter than a full
                    // period so the exfil resumes firing as soon as
                    // the link cools, instead of sitting idle while
                    // the traffic has long since drained.
                    self.nodes[id].role_cooldown = (period / 3).max(1);
                    continue;
                }
                self.nodes[id].role_cooldown = period;
                // Ghost-packet roll: Opportunist and Plague
                // exfils occasionally emit a decoy packet with
                // no payload. Adds load and clogs router caches
                // without a matching intel reward on delivery.
                // Rolled per fire so a busy exfil produces a
                // steady stream of ghosts without drowning the
                // real traffic channel.
                let faction = self.nodes[id].faction;
                let persona = self
                    .personas
                    .get(faction as usize)
                    .copied()
                    .unwrap_or(super::Persona::Opportunist);
                let ghost_chance: f64 = match persona {
                    super::Persona::Opportunist => 0.25,
                    super::Persona::Plague => 0.20,
                    _ => 0.0,
                };
                let is_ghost = ghost_chance > 0.0 && self.rng.gen_bool(ghost_chance);
                self.packets.push(Packet {
                    link_id,
                    pos: (link.path.len() - 1) as u16,
                    ghost: is_ghost,
                });
            } else {
                self.nodes[id].role_cooldown = period;
            }
        }
    }

    /// Hunters cull same-faction infected neighbors on a period.
    /// Each Hunter on cooldown 0 scans its Chebyshev-1 neighborhood
    /// for an infected same-faction node, picks one, forces it into
    /// the Pwned state (so advance_pwned_and_loss will cascade it
    /// away), and flashes. Cuts off strain spread at the cost of a
    /// host — the defensive counter to Plague persona runs.
    pub(super) fn fire_hunter_culls(&mut self) {
        let period = self.cfg.scanner_ping_period; // reuse the scanner timer
        let pwned_flash = self.cfg.pwned_flash_ticks;
        let hunters: Vec<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                if matches!(n.state, State::Alive)
                    && n.role == Role::Hunter
                    && n.role_cooldown == 0
                {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();
        if hunters.is_empty() {
            return;
        }
        // Build pos → id map once so neighbor lookup is O(neighbors).
        let mut pos_to_id: std::collections::HashMap<(i16, i16), NodeId> =
            std::collections::HashMap::with_capacity(self.nodes.len());
        for (i, n) in self.nodes.iter().enumerate() {
            if matches!(n.state, State::Alive) {
                pos_to_id.insert(n.pos, i);
            }
        }
        let mut kills: Vec<(NodeId, (i16, i16))> = Vec::new();
        for hid in hunters {
            self.nodes[hid].role_cooldown = period;
            let (hpos, hfaction) = (self.nodes[hid].pos, self.nodes[hid].faction);
            let mut target: Option<NodeId> = None;
            'outer: for dy in -1i16..=1 {
                for dx in -1i16..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let np = (hpos.0 + dx, hpos.1 + dy);
                    let Some(&nid) = pos_to_id.get(&np) else {
                        continue;
                    };
                    let n = &self.nodes[nid];
                    if n.faction != hfaction {
                        continue;
                    }
                    if self.is_c2(nid) {
                        continue;
                    }
                    if n.infection.is_some()
                        && matches!(n.state, State::Alive)
                        && n.dying_in == 0
                    {
                        target = Some(nid);
                        break 'outer;
                    }
                }
            }
            if let Some(tid) = target {
                let pos = self.nodes[tid].pos;
                self.nodes[tid].infection = None;
                self.nodes[tid].state = State::Pwned {
                    ticks_left: pwned_flash,
                };
                self.nodes[hid].scan_pulse = 6;
                kills.push((tid, pos));
            }
        }
        for (_, pos) in kills {
            self.log_node(pos, "culled by hunter");
        }
    }

    pub(super) fn fire_defender_pulses(&mut self) {
        let period = self.cfg.defender_pulse_period;
        let base_radius = self.cfg.defender_radius;
        let immunity_ticks = self.era_immunity_ticks();
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
        // Defender ids whose pulse actually cured something this
        // tick — each is a candidate to spawn an antibody worm.
        let mut curing_defenders: Vec<NodeId> = Vec::new();
        for (id, dpos) in defenders {
            // Fortress Tier 2 stretches this defender's effective
            // cure radius so fortified factions feel decisively
            // more antiviral as they tech up.
            let faction = self.nodes[id].faction;
            let radius = base_radius + self.tech_effects(faction).defender_radius_bonus;
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
                    let strain = inf.strain;
                    cured_positions.push((n.pos, n.faction));
                    n.infection = None;
                    n.immunity_strain = Some(strain);
                    n.immunity_ticks = immunity_ticks;
                    if !curing_defenders.contains(&id) {
                        curing_defenders.push(id);
                    }
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
        // Antibody worms: each defender that actually cured
        // something this tick rolls a chance to spawn a same-
        // faction antibody worm on one of its outgoing links.
        // The worm travels like a regular worm but cures the
        // target's infection on arrival instead of infecting.
        for did in curing_defenders {
            if !self.rng.gen_bool(0.65) {
                continue;
            }
            let outgoing: Vec<(usize, bool)> = self
                .links
                .iter()
                .enumerate()
                .filter_map(|(li, l)| {
                    if (l.drawn as usize) < l.path.len() {
                        return None;
                    }
                    if l.a == did {
                        Some((li, true))
                    } else if l.b == did {
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
            let len = self.links[link_id].path.len();
            if len < 2 {
                continue;
            }
            let pos = if from_a { 1 } else { (len - 2) as u16 };
            self.worms.push(Worm {
                link_id,
                pos,
                outbound_from_a: from_a,
                strain: 0,
                is_antibody: true,
            });
            let dpos = self.nodes[did].pos;
            self.log_node(dpos, "antibody launched");
        }
    }
}
