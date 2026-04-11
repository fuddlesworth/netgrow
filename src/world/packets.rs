//! Packet and worm transport layer.
//!
//! Exfil packets hop link-by-link toward C2 along the parent chain,
//! worms crawl single links to infect the far endpoint, and
//! `maybe_spawn_worms` fires new worms off active-infected carriers.
//! Split out of `world/mod.rs` so the core tick loop stays small.

use std::collections::{HashMap, HashSet};

use rand::Rng;

use super::{
    octet_pair, BACKBONE_HOT_LINK, BACKBONE_PROMOTION_THRESHOLD, HOT_LINK, Infection,
    InfectionStage, LinkKind, NodeId, Packet, PACKET_LOAD_INCREMENT, Role, State, STRAIN_COUNT,
    WORM_LOAD_INCREMENT, WORM_STEP_INTERVAL, Worm, World,
};

impl World {
    pub(super) fn advance_packets(&mut self) {
        if self.packets.is_empty() {
            return;
        }
        let inbound = self.build_inbound_links();
        // Index fully-drawn cross-links by endpoint for O(1) reroute lookups.
        let mut cross_at: HashMap<NodeId, Vec<usize>> = HashMap::new();
        for (li, l) in self.links.iter().enumerate() {
            if l.kind != LinkKind::Cross {
                continue;
            }
            if (l.drawn as usize) < l.path.len() {
                continue;
            }
            cross_at.entry(l.a).or_default().push(li);
            cross_at.entry(l.b).or_default().push(li);
        }

        let mut keep: Vec<Packet> = Vec::with_capacity(self.packets.len());
        let mut dropped_count: u32 = 0;
        let mut last_drop_pos: Option<(i16, i16)> = None;
        let mut rerouted_count: u32 = 0;
        // Per-link delivery credits accumulated this tick — applied
        // after the packet loop so we don't have to mutate links and
        // packets in the same borrow.
        let mut delivery_credits: HashMap<usize, u16> = HashMap::new();
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
                    *delivery_credits.entry(pkt.link_id).or_default() += 1;
                    continue; // delivered
                }
                // Router absorption — this parent caches most of
                // the packets it sees, relieving the upstream
                // chain, but lets ~35% pass through so backbones
                // still carry traffic past the cache.
                if self.nodes[parent_id].role == Role::Router
                    && self.rng.gen_bool(0.65)
                {
                    self.nodes[parent_id].pulse = 3;
                    *delivery_credits.entry(pkt.link_id).or_default() += 1;
                    continue;
                }
                if let Some(&next_link) = inbound.get(&parent_id) {
                    let next = &self.links[next_link];
                    let next_hot_ceiling = if next.is_backbone {
                        BACKBONE_HOT_LINK
                    } else {
                        HOT_LINK
                    };
                    let primary_usable = !next.path.is_empty()
                        && next.load < next_hot_ceiling
                        && next.quarantined == 0;
                    if primary_usable {
                        pkt.link_id = next_link;
                        pkt.pos = (next.path.len() - 1) as u16;
                        keep.push(pkt);
                        continue;
                    }
                    // Primary route is congested or quarantined — look
                    // for a cross-link bypass: any cross-link touching
                    // `parent_id` whose far endpoint has a cooler
                    // inbound path we can jump onto.
                    let parent_faction = self.nodes[parent_id].faction;
                    let mut rerouted = false;
                    if let Some(crosses) = cross_at.get(&parent_id) {
                        for &cli in crosses {
                            let c = &self.links[cli];
                            let other = if c.a == parent_id { c.b } else { c.a };
                            if !matches!(self.nodes[other].state, State::Alive)
                                || self.nodes[other].dying_in > 0
                            {
                                continue;
                            }
                            if self.nodes[other].faction != parent_faction {
                                continue;
                            }
                            let Some(&alt) = inbound.get(&other) else {
                                continue;
                            };
                            let alt_link = &self.links[alt];
                            let alt_hot = if alt_link.is_backbone {
                                BACKBONE_HOT_LINK
                            } else {
                                HOT_LINK
                            };
                            if alt_link.path.is_empty()
                                || alt_link.load >= alt_hot
                                || alt_link.quarantined > 0
                            {
                                continue;
                            }
                            pkt.link_id = alt;
                            pkt.pos = (alt_link.path.len() - 1) as u16;
                            keep.push(pkt);
                            rerouted_count += 1;
                            rerouted = true;
                            break;
                        }
                    }
                    if !rerouted {
                        dropped_count += 1;
                        last_drop_pos = Some(self.nodes[parent_id].pos);
                    }
                }
                continue;
            }
            pkt.pos -= 1;
            keep.push(pkt);
        }
        self.packets = keep;
        // Apply delivery credits and promote any Parent links that
        // crossed BACKBONE_PROMOTION_THRESHOLD this tick.
        let mut promoted_backbones: Vec<(i16, i16)> = Vec::new();
        for (link_id, credit) in delivery_credits {
            let link = &mut self.links[link_id];
            link.packets_delivered = link.packets_delivered.saturating_add(credit);
            if !link.is_backbone
                && link.kind == LinkKind::Parent
                && link.packets_delivered >= BACKBONE_PROMOTION_THRESHOLD
            {
                link.is_backbone = true;
                let mid_idx = link.path.len() / 2;
                if let Some(pos) = link.path.get(mid_idx).copied() {
                    promoted_backbones.push(pos);
                }
            }
        }
        for pos in promoted_backbones {
            self.log_node(pos, "backbone link forged");
        }
        // Collapse drops into a single log line on heavy bursts so
        // congested cores don't spam the log. Quiet trickles still get
        // a normal per-node line so a single lost packet stays visible.
        if dropped_count >= 3 {
            self.push_log(format!("{} pkts dropped at congested core", dropped_count));
        } else if dropped_count > 0 {
            if let Some(pos) = last_drop_pos {
                self.log_node(pos, "pkt drop");
            }
        }
        if rerouted_count >= 4 {
            self.push_log(format!("{} pkts rerouted via cross-links", rerouted_count));
        }
    }

    pub(super) fn advance_worms(&mut self) {
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
        // (target, incoming_strain, existing_strain) — handled after
        // the move loop so we can mutate target infection state.
        let mut merges: Vec<(NodeId, u8, u8)> = Vec::new();
        // (src_faction, dst_faction) — cross-faction worm crossings
        // bump the rivalry tracker so feuds escalate over time.
        let mut rivalries: Vec<(u8, u8)> = Vec::new();
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
                    let alive_target = matches!(self.nodes[target].state, State::Alive);
                    let allied_block = blocked_by_alliance(self, src, dst);
                    if !c2_set.contains(&target) && alive_target && !allied_block {
                        match self.nodes[target].infection {
                            None => arrivals
                                .push((target, worm.strain, self.nodes[target].pos)),
                            Some(existing) if existing.strain != worm.strain => merges
                                .push((target, worm.strain, existing.strain)),
                            _ => {}
                        }
                        if src != dst {
                            rivalries.push((src, dst));
                        }
                    }
                    continue;
                }
                worm.pos = next as u16;
            } else {
                if worm.pos == 0 {
                    let target = link_a;
                    let src = self.nodes[link_b].faction;
                    let dst = self.nodes[target].faction;
                    let alive_target = matches!(self.nodes[target].state, State::Alive);
                    let allied_block = blocked_by_alliance(self, src, dst);
                    if !c2_set.contains(&target) && alive_target && !allied_block {
                        match self.nodes[target].infection {
                            None => arrivals
                                .push((target, worm.strain, self.nodes[target].pos)),
                            Some(existing) if existing.strain != worm.strain => merges
                                .push((target, worm.strain, existing.strain)),
                            _ => {}
                        }
                        if src != dst {
                            rivalries.push((src, dst));
                        }
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
        // Strain merge: a worm landing on a node already infected
        // with a different strain combines the two. The hybrid
        // inherits the maximum cure_resist of the parents (capped at
        // VETERAN_CURE_RESIST_CAP) and a deterministic strain id
        // derived from the parents so the same combo always picks
        // the same name.
        for (a, b) in rivalries {
            self.bump_rivalry(a, b, 4);
        }
        for (target, incoming, existing) in merges {
            let existing_resist =
                self.nodes[target].infection.map(|i| i.cure_resist).unwrap_or(0);
            let merged_resist = existing_resist
                .saturating_add(1)
                .min(super::VETERAN_CURE_RESIST_CAP);
            let merged_strain =
                ((incoming as u32 + existing as u32 + 1) as usize) % STRAIN_COUNT;
            let mut merged = Infection::seeded(merged_strain as u8, merged_resist);
            merged.veteran_rank = self.nodes[target]
                .infection
                .map(|i| i.veteran_rank)
                .unwrap_or(0)
                .saturating_add(1);
            self.nodes[target].infection = Some(merged);
            let pos = self.nodes[target].pos;
            let (a, b) = octet_pair(pos);
            let name_in = self.strain_name(incoming);
            let name_ex = self.strain_name(existing);
            let name_new = self.strain_name(merged_strain as u8);
            self.push_log(format!(
                "✦ hybrid ✦ {} × {} → {} @ 10.0.{}.{}",
                name_in, name_ex, name_new, a, b
            ));
        }
    }

    pub(super) fn maybe_spawn_worms(&mut self) {
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
}
