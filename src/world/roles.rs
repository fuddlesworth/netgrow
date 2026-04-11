//! Per-role tick behaviors: scanner pings, exfil packet emission,
//! defender pulses, and role-cooldown bookkeeping.
//!
//! Split out of `world/mod.rs` so the role behaviors can live in one
//! place. `advance_role_cooldowns` decays the transient timers that
//! gate every firing; the `fire_*` methods drive the actual role-
//! specific effects the tick loop triggers each step.

use rand::Rng;

use super::{
    NodeId, Packet, Role, State, World, DIRS, HOT_LINK, SCANNER_PULSE_TICKS, WARM_LINK,
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
        }
    }

    pub(super) fn fire_scanner_pings(&mut self) {
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
            let n = &mut self.nodes[id];
            n.role_cooldown = period;
            n.last_ping_tick = now;
            n.last_ping_dir = Some((dx as i8, dy as i8));
            n.scan_pulse = pulse_ticks;
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

    pub(super) fn fire_exfil_packets(&mut self) {
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
                self.packets.push(Packet {
                    link_id,
                    pos: (link.path.len() - 1) as u16,
                });
            } else {
                self.nodes[id].role_cooldown = period;
            }
        }
    }

    pub(super) fn fire_defender_pulses(&mut self) {
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
}
