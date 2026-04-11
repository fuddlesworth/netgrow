//! Virus subsystem: infection progression, patch waves, mutation,
//! and zero-day events.
//!
//! Split out of `world/mod.rs` so the core state machine stays focused
//! on tick orchestration and spawn/cascade logic. Every method here is
//! an `impl World` method that the main `tick` loop calls in order.

use std::collections::HashSet;

use rand::seq::SliceRandom;
use rand::Rng;

use super::{
    octet_pair, Infection, InfectionStage, NodeId, Role, State, World, STRAIN_COUNT,
    VETERAN_CURE_RESIST_CAP, VETERAN_WAVE_THRESHOLD, ZERO_DAY_MIN_ALIVE, ZERO_DAY_OUTBREAK_MAX,
    ZERO_DAY_OUTBREAK_MIN, ZERO_DAY_OUTBREAK_WEIGHT, ZERO_DAY_PATCH_WEIGHT,
};

impl World {
    pub(super) fn advance_patch_waves(&mut self) {
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
        let mut cured: Vec<((i16, i16), u8)> = Vec::new();
        let mut promoted: Vec<((i16, i16), u8, u8)> = Vec::new();
        for n in self.nodes.iter_mut() {
            if n.infection.is_none() {
                continue;
            }
            for &(ox, oy, r) in &geo {
                let dist = (n.pos.0 - ox).abs().max((n.pos.1 - oy).abs());
                // The wave front is a single ring at Chebyshev distance == r.
                // Each node sees the wave exactly once per pass, so a single
                // wave decrements cure_resist by exactly 1.
                if dist == r {
                    let Some(inf) = n.infection.as_mut() else {
                        break;
                    };
                    // Ransomware is immune to patch waves; only
                    // defender pulses can clear it.
                    if inf.is_ransom {
                        break;
                    }
                    if inf.cure_resist <= 1 {
                        let strain = inf.strain;
                        cured.push((n.pos, n.faction));
                        n.infection = None;
                        // Grant strain-specific post-cure immunity.
                        n.immunity_strain = Some(strain);
                        n.immunity_ticks = super::IMMUNITY_DURATION_TICKS;
                        break;
                    } else {
                        inf.cure_resist -= 1;
                        // Veteran promotion: every Nth wave survived
                        // bumps baseline cure_resist by one, so a
                        // strain that keeps hanging on becomes
                        // progressively harder to clear.
                        inf.wave_survivals = inf.wave_survivals.saturating_add(1);
                        if inf.wave_survivals >= VETERAN_WAVE_THRESHOLD
                            && inf.cure_resist < VETERAN_CURE_RESIST_CAP
                        {
                            inf.wave_survivals = 0;
                            inf.cure_resist = inf.cure_resist.saturating_add(1);
                            inf.veteran_rank = inf.veteran_rank.saturating_add(1);
                            promoted.push((n.pos, inf.strain, inf.veteran_rank));
                        }
                    }
                }
            }
        }
        self.patch_waves.retain(|w| w.radius <= max_r);
        for (pos, faction) in cured {
            self.log_node(pos, "cured");
            if let Some(s) = self.faction_stats.get_mut(faction as usize) {
                s.infections_cured += 1;
            }
        }
        for (pos, strain, rank) in promoted {
            let name = self.strain_name(strain);
            let (a, b) = octet_pair(pos);
            self.push_log(format!(
                "{} veteran rank {} @ 10.0.{}.{}",
                name, rank, a, b
            ));
        }
    }

    pub(super) fn advance_infections(&mut self) {
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
                    // Ransomware freezes the host indefinitely instead
                    // of progressing to a terminal crash — the whole
                    // point of the variant.
                    if !inf.is_ransom && inf.age >= incubation + active_len {
                        inf.stage = InfectionStage::Terminal;
                        inf.terminal_ticks = terminal_len;
                    }
                }
                InfectionStage::Terminal => {
                    if inf.is_ransom {
                        // Defensive: if a ransom infection somehow
                        // landed in Terminal, freeze it back to Active.
                        inf.stage = InfectionStage::Active;
                    } else if inf.terminal_ticks <= 1 {
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
        let c2_set: HashSet<NodeId> = self.c2_nodes.iter().copied().collect();
        let adj = self.live_adjacency();
        let mut newly_infected: Vec<(NodeId, u8)> = Vec::new();
        if spread_rate > 0.0 {
            for (id, n) in self.nodes.iter().enumerate() {
                if c2_set.contains(&id) {
                    continue;
                }
                if !matches!(n.state, State::Alive) || n.infection.is_some() {
                    continue;
                }
                // Honeypots stay clean so their disguise survives; defenders
                // are immune by design (they're the antibodies).
                if n.role.is_virus_immune() {
                    continue;
                }
                let Some(neighbors) = adj.get(&id) else {
                    continue;
                };
                let mut tally: [u32; STRAIN_COUNT] = [0; STRAIN_COUNT];
                let mut infected_count: u32 = 0;
                for &m in neighbors {
                    if let Some(inf) = self.nodes[m].infection {
                        if !matches!(inf.stage, InfectionStage::Incubating) {
                            tally[(inf.strain as usize) % STRAIN_COUNT] += 1;
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
                    // Post-cure immunity: if the target is still
                    // immune to this specific strain, the spread
                    // attempt no-ops and the node stays clean.
                    if n.immunity_ticks > 0 && n.immunity_strain == Some(strain) {
                        continue;
                    }
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
            let pos = self.nodes[id].pos;
            let node = &mut self.nodes[id];
            node.infection = None;
            node.state = State::Pwned {
                ticks_left: pwned_flash,
            };
            self.log_node(pos, "necrotic");
        }

        for pos in newly_active {
            self.log_node(pos, "symptomatic");
        }

        // Mythic: PANDEMIC fires once per run if every one of the
        // STRAIN_COUNT strains is simultaneously alive in the mesh.
        if !self.mythic_pandemic_seen {
            let mut seen = [false; STRAIN_COUNT];
            for n in &self.nodes {
                if let Some(inf) = n.infection {
                    seen[(inf.strain as usize) % STRAIN_COUNT] = true;
                }
            }
            if seen.iter().all(|s| *s) {
                self.mythic_pandemic_seen = true;
                self.push_log("✦ MYTHIC ✦ PANDEMIC — all strains active".to_string());
            }
        }
    }

    pub(super) fn maybe_seed_infection(&mut self) {
        if self.cfg.virus_seed_rate <= 0.0 {
            return;
        }
        if self.nodes.iter().any(|n| n.infection.is_some()) {
            return;
        }
        // Plague-persona factions seed about twice as fast as the
        // base rate. We approximate this by boosting the roll by the
        // share of factions running Plague — so a run with one of
        // four Plague factions gets a +25% nudge.
        let plague_share = if self.personas.is_empty() {
            0.0
        } else {
            let n = self
                .personas
                .iter()
                .filter(|p| matches!(p, super::Persona::Plague))
                .count() as f32
                / self.personas.len() as f32;
            n
        };
        let seed_rate = (self.cfg.virus_seed_rate * (1.0 + plague_share)) as f64;
        if !self.rng.gen_bool(seed_rate.min(1.0)) {
            return;
        }
        // Bias candidate selection toward Plague-faction hosts when
        // any exist, so the persona's "viral spreader" identity has
        // a visible effect — Plague factions become outbreak
        // epicenters more often.
        let candidates: Vec<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                if !self.is_c2(i)
                    && matches!(n.state, State::Alive)
                    && !n.role.is_virus_immune()
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
        let id = candidates[self.rng.gen_range(0..candidates.len())];
        let strain = self.rng.gen_range(0..STRAIN_COUNT as u8);
        let cure_resist = self.cfg.virus_cure_resist;
        let is_ransom = self.cfg.ransom_chance > 0.0
            && self.rng.gen_bool(self.cfg.ransom_chance as f64);
        self.nodes[id].infection = Some(if is_ransom {
            Infection::seeded_ransom(strain, cure_resist)
        } else {
            Infection::seeded(strain, cure_resist)
        });
        let pos = self.nodes[id].pos;
        let (a, b) = octet_pair(pos);
        let name = self.strain_name(strain);
        let label = if is_ransom { "ransom" } else { "detected" };
        self.push_log(format!("{} {} at 10.0.{}.{}", name, label, a, b));
    }

    pub(super) fn maybe_mutate(&mut self) {
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
                if self.is_c2(i) {
                    return None;
                }
                if !matches!(n.state, State::Alive) {
                    return None;
                }
                if n.infection.is_some() {
                    return None;
                }
                if n.role.is_mutation_locked() {
                    return None; // specialized roles stay in their lane
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
                Role::Honeypot
                | Role::Defender
                | Role::Tower
                | Role::Beacon
                | Role::Proxy
                | Role::Decoy
                | Role::Router
                | Role::Hunter => continue,
            };
            // Pick uniformly from the first two (the third is the sentinel).
            let new_role = choices[self.rng.gen_range(0..2)];
            let pos = self.nodes[id].pos;
            self.nodes[id].role = new_role;
            self.nodes[id].mutated_flash = 6;
            self.log_node(pos, &format!("mutated → {}", new_role.display_name()));
        }
    }

    pub(super) fn maybe_zero_day(&mut self) {
        // Need enough alive nodes before the roll even becomes
        // meaningful — preserve the existing ZERO_DAY_MIN_ALIVE floor.
        let alive_count = self
            .nodes
            .iter()
            .filter(|n| matches!(n.state, State::Alive))
            .count();
        if alive_count < ZERO_DAY_MIN_ALIVE {
            return;
        }
        if !self.roll_periodic(self.cfg.zero_day_period, self.cfg.zero_day_chance) {
            return;
        }
        // Mythic: zero-day coinciding with an active storm.
        if self.is_storming() {
            self.push_log("✦ MYTHIC ✦ CONFLUENCE — zero-day amid storm".to_string());
        }
        let roll = self.rng.gen::<f32>();
        if roll < ZERO_DAY_OUTBREAK_WEIGHT {
            self.zero_day_outbreak();
        } else if roll < ZERO_DAY_PATCH_WEIGHT {
            self.zero_day_emergency_patch();
        } else {
            self.zero_day_immune_breakthrough();
        }
    }

    pub(super) fn zero_day_outbreak(&mut self) {
        let strain = self.rng.gen_range(0..STRAIN_COUNT as u8);
        let count = self.rng.gen_range(ZERO_DAY_OUTBREAK_MIN..=ZERO_DAY_OUTBREAK_MAX);
        let cure_resist = self.cfg.virus_cure_resist.saturating_mul(2);
        let mut candidates: Vec<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                if !self.is_c2(i)
                    && matches!(n.state, State::Alive)
                    && n.infection.is_none()
                    && !n.role.is_virus_immune()
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
        // Shuffle and take so we hit `count` distinct nodes (or all of them
        // if fewer candidates exist) without picking the same id twice.
        candidates.shuffle(&mut self.rng);
        let take = (count as usize).min(candidates.len());
        for &id in candidates.iter().take(take) {
            self.nodes[id].infection = Some(Infection::seeded(strain, cure_resist));
        }
        let name = self.strain_name(strain);
        self.push_log(format!("ZERO-DAY: {} outbreak ({})", name, take));
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
            "ZERO-DAY: emergency patch ({} cleared)",
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
            "ZERO-DAY: immune boost ({})",
            boosted
        ));
    }

    /// Infect a random Alive non-C2 non-Honeypot node with a fresh strain.
    /// Used by the `i` keybinding and by tests. Refuses to fire when the
    /// virus layer is disabled so --disable-virus really means "off".
    pub fn inject_infection(&mut self) -> Option<NodeId> {
        if self.cfg.virus_spread_rate <= 0.0 && self.cfg.virus_seed_rate <= 0.0 {
            self.push_log("inject refused: virus layer disabled".to_string());
            return None;
        }
        let candidates: Vec<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                if !self.is_c2(i)
                    && matches!(n.state, State::Alive)
                    && n.infection.is_none()
                    && !n.role.is_virus_immune()
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
        let strain = self.rng.gen_range(0..STRAIN_COUNT as u8);
        let cure_resist = self.cfg.virus_cure_resist;
        self.nodes[id].infection = Some(Infection::seeded(strain, cure_resist));
        let pos = self.nodes[id].pos;
        let (a, b) = octet_pair(pos);
        let name = self.strain_name(strain);
        self.push_log(format!("INJECTED {} @ 10.0.{}.{}", name, a, b));
        Some(id)
    }
}
