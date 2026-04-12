use super::*;

#[test]
fn scheduled_subtree_death_eventually_kills_all_descendants() {
    let mut w = World::new(1, (80, 30), Config::default());
    // Kill the RNG-driven loss/spawn so only our scheduled death runs.
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    // Manually build a 3-level tree: c2 -> a -> b -> c
    let a = w.meshes[0].nodes.len();
    w.meshes[0].nodes
        .push(Node::fresh((10, 10), Some(w.primary_c2), 0, Role::Relay, 1));
    let b = w.meshes[0].nodes.len();
    w.meshes[0].nodes.push(Node::fresh((12, 10), Some(a), 0, Role::Relay, 1));
    let c = w.meshes[0].nodes.len();
    w.meshes[0].nodes.push(Node::fresh((14, 10), Some(b), 0, Role::Relay, 1));
    w.schedule_subtree_death(0, a, 1.0);
    // All three descendants should be flagged dying but not yet Dead.
    assert!(w.meshes[0].nodes[a].dying_in > 0);
    assert!(w.meshes[0].nodes[b].dying_in > 0);
    assert!(w.meshes[0].nodes[c].dying_in > 0);
    assert!(matches!(w.meshes[0].nodes[a].state, State::Alive));
    // Run enough ticks to drain the deepest dying_in (distance 2 → delay 7).
    for _ in 0..20 {
        w.tick((80, 30));
    }
    assert!(matches!(w.meshes[0].nodes[a].state, State::Dead));
    assert!(matches!(w.meshes[0].nodes[b].state, State::Dead));
    assert!(matches!(w.meshes[0].nodes[c].state, State::Dead));
    assert!(matches!(w.meshes[0].nodes[w.primary_c2].state, State::Alive));
}

#[test]
fn hardened_node_resists_first_pwn() {
    let mut w = World::new(7, (80, 30), Config::default());
    // Neutralize era modifiers so `p_loss: 1.0` is truly
    // guaranteed — the opening era's 0.7 loss multiplier would
    // otherwise make the single roll probabilistic.
    w.era_rules = EraRules::default();
    w.cfg.p_spawn = 0.0;
    let id = w.meshes[0].nodes.len();
    let mut n = Node::fresh((10, 10), Some(w.primary_c2), 0, Role::Relay, 1);
    n.hardened = true;
    w.meshes[0].nodes.push(n);
    w.cfg.p_loss = 1.0; // force the victim roll to fire
    w.advance_pwned_and_loss(0);
    assert!(matches!(w.meshes[0].nodes[id].state, State::Alive));
    assert!(!w.meshes[0].nodes[id].hardened);
}

#[test]
fn branch_id_inherits_from_parent_not_c2() {
    let mut w = World::new(11, (120, 40), Config::default());
    // First-hop child gets fresh branch id.
    let a = w.alloc_branch_id(0);
    w.meshes[0].nodes
        .push(Node::fresh((30, 10), Some(w.primary_c2), 0, Role::Relay, a));
    let a_id = w.meshes[0].nodes.len() - 1;
    let a_branch = w.meshes[0].nodes[a_id].branch_id;
    w.meshes[0].nodes
        .push(Node::fresh((32, 10), Some(a_id), 0, Role::Relay, a_branch));
    assert_ne!(w.meshes[0].nodes[a_id].branch_id, 0);
    assert_eq!(w.meshes[0].nodes[a_id + 1].branch_id, w.meshes[0].nodes[a_id].branch_id);
}

#[test]
fn packet_reaches_c2_and_drops() {
    let mut w = World::new(3, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    // Build chain c2 -> a -> b (exfil)
    let a = w.meshes[0].nodes.len();
    w.meshes[0].nodes
        .push(Node::fresh((10, 10), Some(w.primary_c2), 0, Role::Relay, 1));
    let b = w.meshes[0].nodes.len();
    w.meshes[0].nodes
        .push(Node::fresh((14, 10), Some(a), 0, Role::Exfil, 1));
    // Manufacture links with full paths marked drawn.
    let path_ca: Vec<(i16, i16)> =
        (w.meshes[0].nodes[w.primary_c2].pos.0..=10).map(|x| (x, 10)).collect();
    let len_ca = path_ca.len() as u16;
    w.meshes[0].links.push(Link {
        a: w.primary_c2,
        b: a,
        path: path_ca,
        drawn: len_ca,
        kind: LinkKind::Parent,
        load: 0,
        breach_ttl: 0,
        burn_ticks: 0,
        quarantined: 0,
        packets_delivered: 0,
        is_backbone: false,
        black_market_until: 0,
        latent: false,
    });
    let path_ab: Vec<(i16, i16)> = (10..=14).map(|x| (x, 10)).collect();
    let len_ab = path_ab.len() as u16;
    w.meshes[0].links.push(Link {
        a,
        b,
        path: path_ab,
        drawn: len_ab,
        kind: LinkKind::Parent,
        load: 0,
        breach_ttl: 0,
        burn_ticks: 0,
        quarantined: 0,
        packets_delivered: 0,
        is_backbone: false,
        black_market_until: 0,
        latent: false,
    });
    // Force the Exfil to fire on tick 0 and then tick enough for the
    // packet to reach C2 and be dropped.
    w.meshes[0].nodes[b].role_cooldown = 0;
    w.fire_exfil_packets(0);
    assert_eq!(w.meshes[0].packets.len(), 1);
    for _ in 0..40 {
        w.advance_packets(0);
    }
    assert!(w.meshes[0].packets.is_empty());
}

#[test]
fn cross_link_saves_reachable_node_from_cascade() {
    let mut w = World::new(5, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    // Diamond: c2 -> a, c2 -> c, a -> b (b in branch_id 1), plus cross b↔c.
    // Kill a. b should die (loses its parent route and isn't cross-linked
    // to anything alive besides c). c is in branch 2 with cross to b, but
    // c has its own parent path to C2, so c must survive. b has no direct
    // parent chain to c after a is gone, so reachability from C2 to b
    // goes c2→c→(cross)→b — b SHOULD survive via the cross.
    let a = w.meshes[0].nodes.len();
    w.meshes[0].nodes
        .push(Node::fresh((20, 10), Some(w.primary_c2), 0, Role::Relay, 1));
    let c = w.meshes[0].nodes.len();
    w.meshes[0].nodes
        .push(Node::fresh((30, 10), Some(w.primary_c2), 0, Role::Relay, 2));
    let b = w.meshes[0].nodes.len();
    w.meshes[0].nodes.push(Node::fresh((25, 12), Some(a), 0, Role::Relay, 1));
    // Fully-drawn cross link b ↔ c.
    let cross_path = vec![(25, 12), (30, 10)]; // cells don't matter for logic
    let len = cross_path.len() as u16;
    w.meshes[0].links.push(Link {
        a: b,
        b: c,
        path: cross_path,
        drawn: len,
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
    let cascade = w.compute_cascade(0, a);
    let ids: HashSet<NodeId> = cascade.iter().map(|(id, _)| *id).collect();
    assert!(ids.contains(&a), "root must be doomed");
    assert!(!ids.contains(&b), "b should survive via cross link to c");
    assert!(!ids.contains(&c), "c has its own route to C2");
}

#[test]
fn shield_flash_is_set_when_hardened_node_is_hit() {
    let mut w = World::new(9, (80, 30), Config::default());
    // Neutralize era modifiers so `p_loss: 1.0` is truly
    // guaranteed — see hardened_node_resists_first_pwn for the
    // same reasoning.
    w.era_rules = EraRules::default();
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 1.0;
    let id = w.meshes[0].nodes.len();
    let mut n = Node::fresh((10, 10), Some(w.primary_c2), 0, Role::Relay, 1);
    n.hardened = true;
    w.meshes[0].nodes.push(n);
    w.advance_pwned_and_loss(0);
    assert!(matches!(w.meshes[0].nodes[id].state, State::Alive));
    assert!(!w.meshes[0].nodes[id].hardened);
    assert!(w.meshes[0].nodes[id].shield_flash > 0, "shield flash should be set");
    // The flash should drain over subsequent ticks.
    w.cfg.p_loss = 0.0; // don't hit it again
    for _ in 0..10 {
        w.tick((80, 30));
    }
    assert_eq!(w.meshes[0].nodes[id].shield_flash, 0);
}

#[test]
fn reconnect_creates_cross_link_between_branches() {
    let mut w = World::new(13, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    w.cfg.reconnect_rate = 1.0;
    w.cfg.reconnect_radius = 20;
    // Two alive nodes in different branches, no existing bridge.
    w.meshes[0].nodes
        .push(Node::fresh((20, 10), Some(w.primary_c2), 0, Role::Relay, 1));
    w.meshes[0].nodes
        .push(Node::fresh((25, 12), Some(w.primary_c2), 0, Role::Relay, 2));
    let before = w.meshes[0].links.iter().filter(|l| l.kind == LinkKind::Cross).count();
    w.maybe_reconnect(0);
    let after = w.meshes[0].links.iter().filter(|l| l.kind == LinkKind::Cross).count();
    assert_eq!(after, before + 1, "should have formed exactly one cross link");
    // Second call should not create a duplicate between the same pair.
    w.maybe_reconnect(0);
    let cross_count = w.meshes[0].links.iter().filter(|l| l.kind == LinkKind::Cross).count();
    assert_eq!(cross_count, after, "must not duplicate existing bridge");
}

#[test]
fn reconnect_refuses_same_branch() {
    let mut w = World::new(17, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    w.cfg.reconnect_rate = 1.0;
    w.cfg.reconnect_radius = 20;
    // Both nodes in the same branch — should NOT form a cross link.
    w.meshes[0].nodes
        .push(Node::fresh((20, 10), Some(w.primary_c2), 0, Role::Relay, 1));
    w.meshes[0].nodes
        .push(Node::fresh((25, 12), Some(w.primary_c2), 0, Role::Relay, 1));
    for _ in 0..20 {
        w.maybe_reconnect(0);
    }
    let cross = w.meshes[0].links.iter().filter(|l| l.kind == LinkKind::Cross).count();
    assert_eq!(cross, 0);
}

#[test]
fn infection_spreads_along_parent_edges() {
    let cfg = Config {
        p_spawn: 0.0,
        p_loss: 0.0,
        virus_seed_rate: 0.0,
        virus_spread_rate: 1.0,
        worm_spawn_rate: 0.0,
        reconnect_rate: 0.0,
        mutate_rate: 0.0,
        ..Config::default()
    };
    let mut w = World::new(21, (80, 30), cfg);
    w.era_rules = EraRules::default();
    // Suppress custom events so a ForcedCascade doesn't kill our
    // test nodes out from under us.
    w.custom_events.clear();
    // Build c2 -> a -> b, infect a and drive it straight to Active so it
    // can infect neighbors.
    let a = w.meshes[0].nodes.len();
    w.meshes[0].nodes
        .push(Node::fresh((10, 10), Some(w.primary_c2), 0, Role::Relay, 1));
    let b = w.meshes[0].nodes.len();
    w.meshes[0].nodes.push(Node::fresh((12, 10), Some(a), 0, Role::Relay, 1));
    w.meshes[0].nodes[a].infection = Some(Infection {
        strain: 3,
        stage: InfectionStage::Active,
        age: w.cfg.virus_incubation_ticks,
        cure_resist: 3,
        terminal_ticks: 0,
        is_ransom: false,
        is_carrier: false,
        wave_survivals: 0,
        veteran_rank: 0,
    });
    // Run a few ticks: spread probability is 1.0 so b should catch
    // it on the very first tick. Give it 20 ticks for margin.
    for _ in 0..20 {
        w.tick((80, 30));
    }
    assert!(
        w.meshes[0].nodes[b].infection.is_some(),
        "b should be infected: state={:?}, alive={}, immunity={}",
        w.meshes[0].nodes[b].state,
        matches!(w.meshes[0].nodes[b].state, State::Alive),
        w.meshes[0].nodes[b].immunity_ticks,
    );
    assert_eq!(w.meshes[0].nodes[b].infection.unwrap().strain, 3);
}

#[test]
fn infection_skips_c2() {
    let mut w = World::new(22, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    w.cfg.virus_seed_rate = 0.0;
    w.cfg.virus_spread_rate = 1.0;
    // Child directly attached to C2, infected and Active.
    let a = w.meshes[0].nodes.len();
    w.meshes[0].nodes
        .push(Node::fresh((10, 10), Some(w.primary_c2), 0, Role::Relay, 1));
    w.meshes[0].nodes[a].infection = Some(Infection {
        strain: 0,
        stage: InfectionStage::Active,
        age: w.cfg.virus_incubation_ticks,
        cure_resist: 3,
        terminal_ticks: 0,
        is_ransom: false,
        is_carrier: false,
        wave_survivals: 0,
        veteran_rank: 0,
    });
    for _ in 0..20 {
        w.tick((80, 30));
    }
    assert!(w.meshes[0].nodes[w.primary_c2].infection.is_none(), "C2 must stay clean");
}

#[test]
fn patch_wave_cures_infected_node_within_radius() {
    let mut w = World::new(24, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    w.cfg.virus_seed_rate = 0.0;
    w.cfg.virus_spread_rate = 0.0;
    w.cfg.worm_spawn_rate = 0.0;
    // Infected node with cure_resist=1, three cells from C2.
    let c2_pos = w.meshes[0].nodes[w.primary_c2].pos;
    let a = w.meshes[0].nodes.len();
    w.meshes[0].nodes.push(Node::fresh(
        (c2_pos.0 + 3, c2_pos.1),
        Some(w.primary_c2),
        0,
        Role::Relay,
        1,
    ));
    w.meshes[0].nodes[a].infection = Some(Infection {
        strain: 0,
        stage: InfectionStage::Incubating,
        age: 0,
        cure_resist: 1,
        terminal_ticks: 0,
        is_ransom: false,
        is_carrier: false,
        wave_survivals: 0,
        veteran_rank: 0,
    });
    // Seed a patch wave directly and tick it forward until the front hits.
    w.meshes[0].patch_waves.push(PatchWave {
        origin: c2_pos,
        radius: 0,
    });
    for _ in 0..10 {
        w.advance_patch_waves(0);
        if w.meshes[0].nodes[a].infection.is_none() {
            break;
        }
    }
    assert!(w.meshes[0].nodes[a].infection.is_none(), "patch wave should cure the node");
}

#[test]
fn worm_delivered_to_alive_neighbor() {
    let mut w = World::new(25, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    w.cfg.virus_seed_rate = 0.0;
    w.cfg.virus_spread_rate = 0.0;
    // Build c2 -> a -> b with fully-drawn links.
    let a = w.meshes[0].nodes.len();
    w.meshes[0].nodes
        .push(Node::fresh((10, 10), Some(w.primary_c2), 0, Role::Relay, 1));
    let b = w.meshes[0].nodes.len();
    w.meshes[0].nodes.push(Node::fresh((14, 10), Some(a), 0, Role::Relay, 1));
    let path_ab: Vec<(i16, i16)> = (10..=14).map(|x| (x, 10)).collect();
    let len_ab = path_ab.len() as u16;
    w.meshes[0].links.push(Link {
        a,
        b,
        path: path_ab,
        drawn: len_ab,
        kind: LinkKind::Parent,
        load: 0,
        breach_ttl: 0,
        burn_ticks: 0,
        quarantined: 0,
        packets_delivered: 0,
        is_backbone: false,
        black_market_until: 0,
        latent: false,
    });
    // Launch a worm from a → b manually and tick the worm advance step
    // enough times for it to reach the far end.
    w.meshes[0].worms.push(Worm {
        link_id: 0,
        pos: 0,
        outbound_from_a: true,
        strain: 2,
        is_antibody: false,
    });
    for _ in 0..10 {
        w.advance_worms(0);
    }
    assert!(w.meshes[0].nodes[b].infection.is_some());
    assert_eq!(w.meshes[0].nodes[b].infection.unwrap().strain, 2);
    assert!(w.meshes[0].worms.is_empty());
}

#[test]
fn terminal_infection_forces_loss() {
    let mut w = World::new(23, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    w.cfg.virus_seed_rate = 0.0;
    w.cfg.virus_spread_rate = 0.0;
    let a = w.meshes[0].nodes.len();
    w.meshes[0].nodes
        .push(Node::fresh((10, 10), Some(w.primary_c2), 0, Role::Relay, 1));
    w.meshes[0].nodes[a].infection = Some(Infection {
        strain: 0,
        stage: InfectionStage::Terminal,
        age: 200,
        cure_resist: 3,
        terminal_ticks: 1,
        is_ransom: false,
        is_carrier: false,
        wave_survivals: 0,
        veteran_rank: 0,
    });
    // One tick drains terminal_ticks and flips to Pwned.
    w.tick((80, 30));
    assert!(matches!(
        w.meshes[0].nodes[a].state,
        State::Pwned { .. } | State::Dead
    ));
    assert!(w.meshes[0].nodes[a].infection.is_none());
}

#[test]
fn mutation_skips_honeypots() {
    let mut w = World::new(26, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    w.cfg.mutate_rate = 1.0;
    w.cfg.mutate_min_age = 0;
    w.cfg.virus_seed_rate = 0.0;
    let id = w.meshes[0].nodes.len();
    w.meshes[0].nodes
        .push(Node::fresh((10, 10), Some(w.primary_c2), 0, Role::Honeypot, 1));
    for _ in 0..10 {
        w.maybe_mutate(0);
    }
    assert_eq!(w.meshes[0].nodes[id].role, Role::Honeypot);
    assert_eq!(w.meshes[0].nodes[id].mutated_flash, 0);
}

#[test]
fn mutation_flips_relay_role_and_flashes() {
    let mut w = World::new(27, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    w.cfg.mutate_rate = 1.0;
    w.cfg.mutate_min_age = 0;
    w.cfg.virus_seed_rate = 0.0;
    let id = w.meshes[0].nodes.len();
    w.meshes[0].nodes
        .push(Node::fresh((10, 10), Some(w.primary_c2), 0, Role::Relay, 1));
    w.maybe_mutate(0);
    assert!(matches!(w.meshes[0].nodes[id].role, Role::Scanner | Role::Exfil));
    assert!(w.meshes[0].nodes[id].mutated_flash > 0);
}

#[test]
fn zero_day_respects_min_node_floor() {
    let mut w = World::new(28, (80, 30), Config::default());
    w.cfg.zero_day_period = 1;
    w.cfg.zero_day_chance = 1.0;
    w.cfg.virus_seed_rate = 0.0;
    // Only C2 alive: 1 node, well below the 10-node minimum.
    w.tick = 1;
    w.maybe_zero_day(0);
    assert!(w.meshes[0].nodes.iter().all(|n| n.infection.is_none()));
}

#[test]
fn zero_day_outbreak_picks_distinct_targets() {
    let mut w = World::new(31, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    w.cfg.virus_seed_rate = 0.0;
    // Push exactly 4 alive candidates so picking 3-5 distinct should
    // saturate the candidate set without ever double-picking.
    for i in 0..4 {
        w.meshes[0].nodes
            .push(Node::fresh((10 + i, 10), Some(w.primary_c2), 0, Role::Relay, 1));
    }
    w.zero_day_outbreak(0);
    let infected = w
        .meshes[0]
        .nodes
        .iter()
        .filter(|n| n.infection.is_some())
        .count();
    // Must hit at least 3 (the configured min) and never exceed 4 (the
    // candidate ceiling). Pre-fix this could come back as 1-2 if the
    // RNG happened to pick duplicates.
    assert!(
        (3..=4).contains(&infected),
        "expected 3-4 distinct infections, got {}",
        infected
    );
}

#[test]
fn infection_spread_skips_honeypots() {
    let mut w = World::new(32, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    w.cfg.virus_seed_rate = 0.0;
    w.cfg.virus_spread_rate = 1.0;
    // Active-infected relay next to a honeypot. Spread fires every tick
    // but the honeypot must remain clean to keep its disguise.
    let infected = w.meshes[0].nodes.len();
    w.meshes[0].nodes
        .push(Node::fresh((10, 10), Some(w.primary_c2), 0, Role::Relay, 1));
    let honey = w.meshes[0].nodes.len();
    w.meshes[0].nodes
        .push(Node::fresh((12, 10), Some(infected), 0, Role::Honeypot, 1));
    w.meshes[0].nodes[infected].infection = Some(Infection {
        strain: 0,
        stage: InfectionStage::Active,
        age: w.cfg.virus_incubation_ticks,
        cure_resist: 4,
        terminal_ticks: 0,
        is_ransom: false,
        is_carrier: false,
        wave_survivals: 0,
        veteran_rank: 0,
    });
    for _ in 0..20 {
        w.tick((80, 30));
    }
    assert!(w.meshes[0].nodes[honey].infection.is_none(), "honeypot must stay clean");
}

#[test]
fn defender_pulse_cures_nearby_infection() {
    let mut w = World::new(33, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    w.cfg.virus_seed_rate = 0.0;
    w.cfg.virus_spread_rate = 0.0;
    w.cfg.defender_pulse_period = 1; // fire on every tick
    w.cfg.defender_radius = 5;
    w.cfg.virus_cure_resist = 1; // single-pulse cure
    let _defender = w.meshes[0].nodes.len();
    w.meshes[0].nodes
        .push(Node::fresh((10, 10), Some(w.primary_c2), 0, Role::Defender, 1));
    let victim = w.meshes[0].nodes.len();
    w.meshes[0].nodes
        .push(Node::fresh((12, 11), Some(w.primary_c2), 0, Role::Relay, 1));
    w.meshes[0].nodes[victim].infection = Some(Infection {
        strain: 0,
        stage: InfectionStage::Active,
        age: w.cfg.virus_incubation_ticks,
        cure_resist: 1,
        terminal_ticks: 0,
        is_ransom: false,
        is_carrier: false,
        wave_survivals: 0,
        veteran_rank: 0,
    });
    w.fire_defender_pulses(0);
    assert!(w.meshes[0].nodes[victim].infection.is_none(), "defender should clear infection in radius");
}

#[test]
fn defender_immune_to_infection() {
    let mut w = World::new(34, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    w.cfg.virus_seed_rate = 0.0;
    w.cfg.virus_spread_rate = 1.0;
    let infected = w.meshes[0].nodes.len();
    w.meshes[0].nodes
        .push(Node::fresh((10, 10), Some(w.primary_c2), 0, Role::Relay, 1));
    let defender = w.meshes[0].nodes.len();
    w.meshes[0].nodes
        .push(Node::fresh((12, 10), Some(infected), 0, Role::Defender, 1));
    w.meshes[0].nodes[infected].infection = Some(Infection {
        strain: 0,
        stage: InfectionStage::Active,
        age: w.cfg.virus_incubation_ticks,
        cure_resist: 4,
        terminal_ticks: 0,
        is_ransom: false,
        is_carrier: false,
        wave_survivals: 0,
        veteran_rank: 0,
    });
    for _ in 0..20 {
        w.tick((80, 30));
    }
    assert!(w.meshes[0].nodes[defender].infection.is_none(), "defender should never get infected");
}

#[test]
fn multiple_c2s_each_get_distinct_factions() {
    let cfg = Config {
        c2_count: 3,
        p_spawn: 0.0,
        ..Config::default()
    };
    let w = World::new(40, (120, 30), cfg);
    assert_eq!(w.meshes[0].c2_nodes.len(), 3);
    assert_eq!(w.meshes[0].nodes[w.meshes[0].c2_nodes[0]].faction, 0);
    assert_eq!(w.meshes[0].nodes[w.meshes[0].c2_nodes[1]].faction, 1);
    assert_eq!(w.meshes[0].nodes[w.meshes[0].c2_nodes[2]].faction, 2);
    // Random placement with minimum spacing — every pair must be
    // at a distinct cell.
    for i in 0..w.meshes[0].c2_nodes.len() {
        for j in (i + 1)..w.meshes[0].c2_nodes.len() {
            assert_ne!(
                w.meshes[0].nodes[w.meshes[0].c2_nodes[i]].pos,
                w.meshes[0].nodes[w.meshes[0].c2_nodes[j]].pos,
                "C2s {} and {} overlap",
                i,
                j
            );
        }
    }
}

#[test]
fn cascade_does_not_kill_other_factions() {
    let cfg = Config {
        c2_count: 2,
        p_spawn: 0.0,
        p_loss: 0.0,
        virus_seed_rate: 0.0,
        ..Config::default()
    };
    let mut w = World::new(41, (80, 30), cfg);
    // Build one child for each faction.
    let f0 = w.meshes[0].c2_nodes[0];
    let f1 = w.meshes[0].c2_nodes[1];
    let child0 = w.meshes[0].nodes.len();
    let mut n0 = Node::fresh((10, 10), Some(f0), 0, Role::Relay, 1);
    n0.faction = 0;
    w.meshes[0].nodes.push(n0);
    let child1 = w.meshes[0].nodes.len();
    let mut n1 = Node::fresh((40, 10), Some(f1), 0, Role::Relay, 2);
    n1.faction = 1;
    w.meshes[0].nodes.push(n1);
    // Trigger a cascade on faction 0's child. Faction 1 must survive.
    w.schedule_subtree_death(0, child0, 1.0);
    for _ in 0..20 {
        w.tick((80, 30));
    }
    assert!(matches!(w.meshes[0].nodes[child0].state, State::Dead));
    assert!(matches!(w.meshes[0].nodes[child1].state, State::Alive));
    assert!(matches!(w.meshes[0].nodes[f1].state, State::Alive));
}

#[test]
fn day_night_cycle_flips_at_half_period() {
    let cfg = Config {
        day_night_period: 100,
        ..Config::default()
    };
    let mut w = World::new(50, (80, 30), cfg);
    assert!(!w.is_night(), "starts in day phase");
    w.tick = 49;
    assert!(!w.is_night(), "still day just before midpoint");
    w.tick = 50;
    assert!(w.is_night(), "night at midpoint");
    w.tick = 99;
    assert!(w.is_night(), "still night at period end");
    w.tick = 100;
    assert!(!w.is_night(), "day at next period start");
}

#[test]
fn link_load_accumulates_and_decays() {
    let cfg = Config {
        p_spawn: 0.0,
        p_loss: 0.0,
        virus_seed_rate: 0.0,
        ..Config::default()
    };
    let mut w = World::new(60, (80, 30), cfg);
    let a = w.meshes[0].nodes.len();
    w.meshes[0].nodes
        .push(Node::fresh((10, 10), Some(w.primary_c2), 0, Role::Relay, 1));
    let b = w.meshes[0].nodes.len();
    w.meshes[0].nodes.push(Node::fresh((14, 10), Some(a), 0, Role::Exfil, 1));
    let path: Vec<(i16, i16)> = (10..=14).map(|x| (x, 10)).collect();
    let len = path.len() as u16;
    w.meshes[0].links.push(Link {
        a,
        b,
        path,
        drawn: len,
        kind: LinkKind::Parent,
        load: 0,
        breach_ttl: 0,
        burn_ticks: 0,
        quarantined: 0,
        packets_delivered: 0,
        is_backbone: false,
        black_market_until: 0,
        latent: false,
    });
    // Park a packet on the link and tick the motion phase a few times.
    w.meshes[0].packets.push(Packet {
        link_id: 0,
        pos: len - 1,
        ghost: false,
    });
    for _ in 0..5 {
        w.decay_link_load(0);
        w.advance_packets(0);
    }
    assert!(w.meshes[0].links[0].load > 0, "load should accumulate from in-flight packet");

    // Stop feeding packets; load decays back to zero.
    w.meshes[0].packets.clear();
    for _ in 0..20 {
        w.decay_link_load(0);
    }
    assert_eq!(w.meshes[0].links[0].load, 0, "load should decay to zero");
}

#[test]
fn honeypot_trip_reveals_backdoor_links() {
    let cfg = Config {
        p_spawn: 0.0,
        p_loss: 0.0,
        virus_seed_rate: 0.0,
        honeypot_backdoor_max: 3,
        honeypot_backdoor_radius: 20,
        ..Config::default()
    };
    let mut w = World::new(70, (80, 30), cfg);
    // Honeypot in its own branch, plus three alive neighbors in
    // separate branches within the backdoor radius.
    let honey = w.meshes[0].nodes.len();
    let mut h = Node::fresh((20, 10), Some(w.primary_c2), 0, Role::Honeypot, 1);
    h.faction = 0;
    w.meshes[0].nodes.push(h);
    for (i, pos) in [(25, 10), (18, 15), (22, 12)].iter().enumerate() {
        let mut n = Node::fresh(*pos, Some(w.primary_c2), 0, Role::Relay, 2 + i as u16);
        n.faction = 0;
        w.meshes[0].nodes.push(n);
    }
    let before = w
        .meshes[0]
        .links
        .iter()
        .filter(|l| l.kind == LinkKind::Cross)
        .count();
    w.reveal_honeypot_backdoors(0, honey);
    let after = w
        .meshes[0]
        .links
        .iter()
        .filter(|l| l.kind == LinkKind::Cross)
        .count();
    assert!(
        after > before,
        "expected at least one backdoor cross-link to be added; before={} after={}",
        before,
        after
    );
    // All new cross-links should originate from the honeypot.
    let from_honey = w
        .meshes[0]
        .links
        .iter()
        .filter(|l| l.kind == LinkKind::Cross && l.a == honey)
        .count();
    assert_eq!(
        from_honey,
        after - before,
        "all revealed backdoors should anchor on the honeypot"
    );
}

#[test]
fn tower_absorbs_pwn_attempts_before_dying() {
    let cfg = Config {
        p_spawn: 0.0,
        p_loss: 1.0,
        virus_seed_rate: 0.0,
        tower_pwn_resist: 2,
        ..Config::default()
    };
    let mut w = World::new(80, (80, 30), cfg);
    // Neutralize era modifiers so `p_loss: 1.0` is truly guaranteed —
    // otherwise the opening era's loss multiplier makes the 3-tick
    // guarantee probabilistic.
    w.era_rules = EraRules::default();
    // One tower adjacent to C2 — the only possible victim.
    let tower = w.meshes[0].nodes.len();
    let mut n = Node::fresh((11, 10), Some(w.primary_c2), 0, Role::Tower, 1);
    n.pwn_resist = 2;
    n.faction = 0;
    w.meshes[0].nodes.push(n);
    // Three ticks of guaranteed loss rolls. First two should consume
    // the pwn_resist charges; third should finally pwn the tower.
    for _ in 0..2 {
        w.tick((80, 30));
        assert!(
            matches!(w.meshes[0].nodes[tower].state, State::Alive),
            "tower should still be alive"
        );
    }
    w.tick((80, 30));
    assert!(
        matches!(w.meshes[0].nodes[tower].state, State::Pwned { .. } | State::Dead),
        "tower should be down after 3 hits"
    );
}

#[test]
fn tech_research_accrues_and_unlocks_tier_1() {
    let cfg = Config {
        p_spawn: 0.0,
        p_loss: 0.0,
        virus_seed_rate: 0.0,
        worm_spawn_rate: 0.0,
        reconnect_rate: 0.0,
        c2_count: 2,
        epoch_period: 0,
        ..Config::default()
    };
    let mut w = World::new(1, (80, 30), cfg);
    // Ensure both factions start at tier 0 and 0 research.
    for stats in &w.faction_stats {
        assert_eq!(stats.tech_tier, 0);
        assert_eq!(stats.research, 0);
    }
    // Hand-credit intel so the delta-based income term kicks in
    // and pushes F0 past the Tier 1 threshold on the next pass.
    // Intel contributes 1:1 under the retuned formula, so we
    // need at least TECH_TIER_1_COST worth of delta to unlock.
    w.faction_stats[0].intel = TECH_TIER_1_COST + 20;
    // Force a single research pass directly — tick() only runs
    // it on the sample cadence, and we want a deterministic check.
    w.advance_research();
    assert!(w.faction_stats[0].research >= TECH_TIER_1_COST,
        "expected F0 research >= {}, got {}",
        TECH_TIER_1_COST,
        w.faction_stats[0].research);
    assert!(w.faction_stats[0].tech_tier >= 1);
    // Log line should have been emitted.
    let tech_lines = w
        .logs
        .iter()
        .filter(|(s, _)| s.starts_with("✦ tech"))
        .count();
    assert!(tech_lines >= 1);
}

#[test]
fn tech_role_intensity_amplifies_persona_at_tier_1_plus() {
    let mut w = World::new(1, (80, 30), Config::default());
    // Baseline: tier 0 → intensity 1.0.
    assert_eq!(w.tech_effects(0).role_intensity, 1.0);
    // Bump tier directly and confirm the intensity climbs.
    w.faction_stats[0].tech_tier = 1;
    assert!((w.tech_effects(0).role_intensity - 1.35).abs() < 1e-6);
    w.faction_stats[0].tech_tier = 2;
    assert!((w.tech_effects(0).role_intensity - 1.6).abs() < 1e-6);
    w.faction_stats[0].tech_tier = 3;
    assert!((w.tech_effects(0).role_intensity - 1.6).abs() < 1e-6);
}

#[test]
fn tech_persona_passives_gate_on_tier_and_match() {
    let mut w = World::new(1, (80, 30), Config::default());
    // T0: no bonuses anywhere.
    let t0 = w.tech_effects(0);
    assert_eq!(t0.defender_radius_bonus, 0);
    assert_eq!(t0.scanner_period_mult, 1.0);
    assert_eq!(t0.worm_spawn_mult, 1.0);
    assert_eq!(t0.bridge_mult, 1.0);
    // T2 Fortress → defender radius bonus.
    w.faction_stats[0].tech_tier = 2;
    w.personas[0] = Persona::Fortress;
    let t2_fortress = w.tech_effects(0);
    assert_eq!(t2_fortress.defender_radius_bonus, 2);
    assert_eq!(t2_fortress.worm_spawn_mult, 1.0);
    // T2 Plague → worm spawn bonus instead.
    w.personas[0] = Persona::Plague;
    let t2_plague = w.tech_effects(0);
    assert_eq!(t2_plague.defender_radius_bonus, 0);
    assert!((t2_plague.worm_spawn_mult - 2.0).abs() < 1e-6);
    // T2 Aggressor → scanner period cut.
    w.personas[0] = Persona::Aggressor;
    assert!((w.tech_effects(0).scanner_period_mult - 0.65).abs() < 1e-6);
    // T2 Opportunist → bridge mult.
    w.personas[0] = Persona::Opportunist;
    assert!((w.tech_effects(0).bridge_mult - 2.0).abs() < 1e-6);
}

#[test]
fn diplomacy_pressure_escalates_to_cold_war_then_open_war() {
    let cfg = Config {
        p_spawn: 0.0,
        p_loss: 0.0,
        virus_seed_rate: 0.0,
        worm_spawn_rate: 0.0,
        reconnect_rate: 0.0,
        c2_count: 2,
        epoch_period: 0,
        ..Config::default()
    };
    let mut w = World::new(1, (80, 30), cfg);
    // Two factions should be alive.
    assert_eq!(w.meshes[0].c2_nodes.len(), 2);
    // Push pressure past COLD_WAR_THRESHOLD and let the state
    // machine run one pass.
    w.bump_rivalry(0, 1, COLD_WAR_THRESHOLD);
    w.advance_diplomacy();
    assert_eq!(w.relation_state(0, 1), DiplomaticState::ColdWar);
    // Now push past the war threshold — the cold-war pass should
    // promote it to OpenWar.
    w.bump_rivalry(0, 1, WAR_DECLARATION_THRESHOLD);
    w.advance_diplomacy();
    assert_eq!(w.relation_state(0, 1), DiplomaticState::OpenWar);
    assert!(w.at_war(0, 1));
    // Order-insensitive lookups.
    assert!(w.at_war(1, 0));
    assert_eq!(w.relation_state(1, 0), DiplomaticState::OpenWar);
}

#[test]
fn defector_flips_faction_and_credits_intel() {
    let cfg = Config {
        p_spawn: 0.0,
        p_loss: 0.0,
        virus_seed_rate: 0.0,
        c2_count: 2,
        defector_period: 1, // fire every tick
        defector_chance: 1.0,
        ..Config::default()
    };
    let mut w = World::new(5, (80, 30), cfg);
    // Plant a single alive F0 node that's the only possible
    // defection candidate so we can assert its fate deterministically.
    let target = w.meshes[0].nodes.len();
    let mut n = Node::fresh((20, 15), Some(w.primary_c2), 0, Role::Relay, 1);
    n.faction = 0;
    w.meshes[0].nodes.push(n);
    let f1_intel_before = w.faction_stats[1].intel;
    let reward = w.cfg.defector_intel_reward;
    // roll_periodic returns false at tick 0, so advance one tick
    // before firing the defector roll.
    w.tick = 1;
    w.maybe_defector(0);
    // Node should have flipped to F1 (the only rival with an
    // alive C2).
    assert_eq!(w.meshes[0].nodes[target].faction, 1);
    // F1 should have received the intel reward.
    assert_eq!(w.faction_stats[1].intel, f1_intel_before + reward);
    // Parent should point at an alive F1 node (either the C2
    // or a same-faction sibling).
    let parent = w.meshes[0].nodes[target].parent.expect("defector should have parent");
    assert_eq!(w.meshes[0].nodes[parent].faction, 1);
    // Mythic log fired.
    let logged = w
        .logs
        .iter()
        .any(|(s, _)| s.contains("defector"));
    assert!(logged);
}

#[test]
fn ghost_packet_delivers_without_crediting_intel() {
    let mut w = World::new(3, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    // Build c2 → a chain and park a ghost packet right at the
    // parent-end of the c2→a link so the next advance_packets
    // call drops it into C2.
    let a = w.meshes[0].nodes.len();
    w.meshes[0].nodes
        .push(Node::fresh((10, 10), Some(w.primary_c2), 0, Role::Relay, 1));
    let path_ca: Vec<(i16, i16)> =
        (w.meshes[0].nodes[w.primary_c2].pos.0..=10).map(|x| (x, 10)).collect();
    let len_ca = path_ca.len() as u16;
    w.meshes[0].links.push(Link {
        a: w.primary_c2,
        b: a,
        path: path_ca,
        drawn: len_ca,
        kind: LinkKind::Parent,
        load: 0,
        breach_ttl: 0,
        burn_ticks: 0,
        quarantined: 0,
        packets_delivered: 0,
        is_backbone: false,
        black_market_until: 0,
        latent: false,
    });
    let intel_before = w.faction_stats[0].intel;
    // Park a ghost packet at pos 0 — ready to drop into C2.
    w.meshes[0].packets.push(Packet {
        link_id: 0,
        pos: 0,
        ghost: true,
    });
    w.advance_packets(0);
    // Packet should have been delivered (removed from the vec).
    assert!(w.meshes[0].packets.is_empty());
    // Intel should NOT have incremented — ghost packets skip the
    // reward path on C2 delivery.
    assert_eq!(w.faction_stats[0].intel, intel_before);
}

#[test]
fn custom_events_roll_at_world_creation_and_fire_on_trigger() {
    let mut w = World::new(42, (80, 30), Config::default());
    // World::new should roll 3-5 custom events.
    assert!(w.custom_events.len() >= 3 && w.custom_events.len() <= 5);
    // Each event has a non-empty name and zero fire count.
    for ev in &w.custom_events {
        assert!(!ev.name.is_empty(), "event name should be non-empty");
        assert_eq!(ev.fire_count, 0);
        assert_eq!(ev.last_fired_tick, 0);
    }
    // Force every event into a permissive state: trigger
    // EverySample, condition Always. That way advance_custom_events
    // must fire at least one on the next call (subject to per-event
    // cooldown which is 0 at the start).
    for ev in w.custom_events.iter_mut() {
        ev.trigger = EventTrigger::EverySample;
        ev.condition = EventCondition::Always;
    }
    w.tick = 100;
    w.advance_custom_events();
    let any_fired = w.custom_events.iter().any(|e| e.fire_count > 0);
    assert!(any_fired, "at least one event should have fired under permissive state");
    // A mythic log line should have been pushed.
    let mythic_logged = w
        .logs
        .iter()
        .any(|(s, _)| s.starts_with("✦ MYTHIC ✦"));
    assert!(mythic_logged);
}

#[test]
fn custom_events_respect_cooldown() {
    let mut w = World::new(7, (80, 30), Config::default());
    // Collapse to one event with a known cooldown and permissive
    // trigger/condition.
    w.custom_events.clear();
    w.custom_events.push(CustomEvent {
        name: "test event".to_string(),
        trigger: EventTrigger::EverySample,
        condition: EventCondition::Always,
        effect: EventEffect::IntelBonusToAll,
        cooldown_ticks: 500,
        last_fired_tick: 0,
        fire_count: 0,
    });
    // First call fires it.
    w.tick = 10;
    w.advance_custom_events();
    assert_eq!(w.custom_events[0].fire_count, 1);
    // Second call well within cooldown — should NOT re-fire.
    w.tick = 200;
    w.advance_custom_events();
    assert_eq!(w.custom_events[0].fire_count, 1);
    // After cooldown elapses, it fires again.
    w.tick = 600;
    w.advance_custom_events();
    assert_eq!(w.custom_events[0].fire_count, 2);
}

#[test]
fn custom_events_are_deterministic_per_seed() {
    let w1 = World::new(123, (80, 30), Config::default());
    let w2 = World::new(123, (80, 30), Config::default());
    assert_eq!(w1.custom_events.len(), w2.custom_events.len());
    for (a, b) in w1.custom_events.iter().zip(w2.custom_events.iter()) {
        assert_eq!(a.name, b.name);
    }
    // Different seed produces different event set (at least one
    // name differs).
    let w3 = World::new(999, (80, 30), Config::default());
    let any_differ = w1
        .custom_events
        .iter()
        .zip(w3.custom_events.iter())
        .any(|(a, b)| a.name != b.name);
    assert!(any_differ, "different seeds should produce different event names");
}

#[test]
fn drought_tightens_effective_hot_link_while_active() {
    let mut w = World::new(1, (80, 30), Config::default());
    // Baseline: the effective hot ceiling matches HOT_LINK for a
    // non-backbone link and BACKBONE_HOT_LINK for backbones.
    let link = Link {
        a: 0,
        b: 0,
        path: vec![],
        drawn: 0,
        kind: LinkKind::Parent,
        load: 0,
        breach_ttl: 0,
        burn_ticks: 0,
        quarantined: 0,
        packets_delivered: 0,
        is_backbone: false,
        black_market_until: 0,
        latent: false,
    };
    assert!(!w.is_droughted());
    assert_eq!(w.effective_hot_link(&link), HOT_LINK);
    // Activate a drought and confirm the ceiling drops by the
    // configured penalty.
    w.meshes[0].drought_until = w.tick + 100;
    assert!(w.is_droughted());
    assert_eq!(
        w.effective_hot_link(&link),
        HOT_LINK - w.cfg.drought_hot_penalty
    );
    // Backbone links get the same penalty off their inflated base.
    let backbone = Link {
        is_backbone: true,
        black_market_until: 0,
        ..link
    };
    assert_eq!(
        w.effective_hot_link(&backbone),
        BACKBONE_HOT_LINK - w.cfg.drought_hot_penalty
    );
}

#[test]
fn fission_splits_a_divergent_branch_into_new_faction() {
    let cfg = Config {
        p_spawn: 0.0,
        p_loss: 0.0,
        virus_seed_rate: 0.0,
        worm_spawn_rate: 0.0,
        reconnect_rate: 0.0,
        c2_count: 1,
        epoch_period: 0,
        ..Config::default()
    };
    let mut w = World::new(1, (80, 30), cfg);
    // Force F0 into Plague persona. Plague's divergent roles are
    // [Defender, Tower, Hunter] — we'll seed a branch composed
    // mostly of Defenders so the branch reads as a Fortress split.
    w.personas[0] = Persona::Plague;
    // Plant a 20-node branch of Defender+Tower nodes on branch_id 7,
    // faction 0, all parented (conceptually) to the C2. That's
    // 100% divergent roles for a Plague faction, well past the
    // 50% threshold.
    let branch_id = 7u16;
    for x in 10..30 {
        let mut n = Node::fresh((x, 12), Some(w.primary_c2), 0, Role::Defender, branch_id);
        n.faction = 0;
        w.meshes[0].nodes.push(n);
    }
    let initial_c2_count = w.meshes[0].c2_nodes.len();
    // Run the fission pass directly. Roll chance is 0.18 so we
    // may need multiple attempts; drive it deterministically by
    // looping up to a generous ceiling.
    let mut split = false;
    for _ in 0..200 {
        if w.advance_fission() > 0 {
            split = true;
            break;
        }
    }
    assert!(split, "fission should have fired on a 20-defender Plague branch");
    assert!(w.meshes[0].c2_nodes.len() > initial_c2_count);
    // New faction should be Fortress (signature role = Defender).
    // The new faction id is the global count before the push,
    // not the mesh-local c2_nodes count.
    let new_faction_id = (w.faction_stats.len() - 1) as u8;
    assert_eq!(w.personas[new_faction_id as usize], Persona::Fortress);
    // OpenWar relation should be live with high pressure.
    let rel = w.relation(0, new_faction_id);
    assert_eq!(rel.state, DiplomaticState::OpenWar);
    assert!(rel.pressure >= FISSION_INITIAL_PRESSURE);
    // The splinter faction's hub has C2 HP and no parent.
    // Find the hub by scanning the mesh's c2_nodes for the
    // one owned by the new faction.
    let hub = *w.meshes[0]
        .c2_nodes
        .iter()
        .find(|&&cid| w.meshes[0].nodes[cid].faction == new_faction_id)
        .expect("splinter should have a C2 on mesh 0");
    assert_eq!(w.meshes[0].nodes[hub].pwn_resist, C2_INITIAL_HP);
    assert!(w.meshes[0].nodes[hub].parent.is_none());
    // The war mythic log line should have fired.
    let fission_logged = w
        .logs
        .iter()
        .any(|(s, _)| s.starts_with("✦ MYTHIC ✦") && s.contains("splinters from"));
    assert!(fission_logged);
}

#[test]
fn fission_ignores_opportunist_factions() {
    let cfg = Config {
        p_spawn: 0.0,
        p_loss: 0.0,
        virus_seed_rate: 0.0,
        c2_count: 1,
        epoch_period: 0,
        ..Config::default()
    };
    let mut w = World::new(1, (80, 30), cfg);
    w.personas[0] = Persona::Opportunist;
    // Plant a 20-node branch of Defenders — would fission under
    // any ideological persona, but Opportunist has no identity
    // to diverge from so fission should never fire.
    for x in 10..30 {
        let mut n = Node::fresh((x, 12), Some(w.primary_c2), 0, Role::Defender, 9);
        n.faction = 0;
        w.meshes[0].nodes.push(n);
    }
    let initial_c2_count = w.meshes[0].c2_nodes.len();
    for _ in 0..500 {
        w.advance_fission();
    }
    assert_eq!(
        w.meshes[0].c2_nodes.len(),
        initial_c2_count,
        "Opportunist factions should never fission"
    );
}

#[test]
fn fission_ignores_branches_below_size_threshold() {
    let cfg = Config {
        p_spawn: 0.0,
        p_loss: 0.0,
        virus_seed_rate: 0.0,
        c2_count: 1,
        epoch_period: 0,
        ..Config::default()
    };
    let mut w = World::new(1, (80, 30), cfg);
    w.personas[0] = Persona::Plague;
    // Plant a 5-node divergent branch — below FISSION_MIN_BRANCH_SIZE.
    for x in 10..15 {
        let mut n = Node::fresh((x, 12), Some(w.primary_c2), 0, Role::Defender, 3);
        n.faction = 0;
        w.meshes[0].nodes.push(n);
    }
    let initial_c2_count = w.meshes[0].c2_nodes.len();
    for _ in 0..500 {
        w.advance_fission();
    }
    assert_eq!(
        w.meshes[0].c2_nodes.len(),
        initial_c2_count,
        "branches below FISSION_MIN_BRANCH_SIZE should never fission"
    );
}

#[test]
fn mercenary_auction_flips_unaffiliated_node_to_richest_bidder() {
    let cfg = Config {
        p_spawn: 0.0,
        p_loss: 0.0,
        virus_seed_rate: 0.0,
        c2_count: 2,
        ..Config::default()
    };
    let mut w = World::new(1, (80, 30), cfg);
    // Plant a mercenary node and give F1 enough intel to win.
    let merc = w.meshes[0].nodes.len();
    let mut n = Node::fresh((20, 15), None, 0, Role::Relay, 99);
    n.faction = MERCENARY_FACTION;
    w.meshes[0].nodes.push(n);
    w.faction_stats[0].intel = MERCENARY_MIN_BID_INTEL;
    w.faction_stats[1].intel = MERCENARY_MIN_BID_INTEL + 50;
    // Auction runs on a period cadence — advance tick to a
    // multiple of MERCENARY_AUCTION_PERIOD.
    w.tick = MERCENARY_AUCTION_PERIOD;
    w.maybe_mercenary_auction(0);
    // Mercenary should have flipped to F1 (richer bidder).
    assert_eq!(w.meshes[0].nodes[merc].faction, 1);
    // F1 paid the bid cost.
    assert_eq!(
        w.faction_stats[1].intel,
        MERCENARY_MIN_BID_INTEL + 50 - MERCENARY_BID_COST
    );
}

#[test]
fn extinction_triggers_reseed_after_silent_interval() {
    let cfg = Config {
        p_spawn: 0.0,
        p_loss: 0.0,
        virus_seed_rate: 0.0,
        worm_spawn_rate: 0.0,
        reconnect_rate: 0.0,
        c2_count: 2,
        c2_count_max: 3,
        epoch_period: 0,
        ..Config::default()
    };
    let mut w = World::new(7, (80, 30), cfg);
    let initial_c2_count = w.meshes[0].c2_nodes.len();
    // Kill every node so the mesh is fully extinct.
    for n in w.meshes[0].nodes.iter_mut() {
        n.state = State::Dead;
    }
    // First detection should flag extinction and log the mythic.
    w.check_extinction_and_reseed();
    assert!(
        w.meshes[0].extinction_since_tick.is_some(),
        "extinction should be detected on first all-dead check"
    );
    let extinction_logged = w
        .logs
        .iter()
        .any(|(s, _)| s.starts_with("✦ MYTHIC ✦ EXTINCTION"));
    assert!(extinction_logged, "extinction mythic line should fire");
    // No reseed yet — cooldown hasn't elapsed.
    assert_eq!(w.meshes[0].c2_nodes.len(), initial_c2_count);
    // Fast-forward past the cooldown and trigger the check again.
    w.tick = EXTINCTION_RESEED_DELAY_TICKS + 10;
    w.check_extinction_and_reseed();
    // A fresh cohort should now be appended and the timer cleared.
    assert!(
        w.meshes[0].c2_nodes.len() > initial_c2_count,
        "reseed should have appended fresh C2s: got {} (was {})",
        w.meshes[0].c2_nodes.len(),
        initial_c2_count
    );
    assert!(w.meshes[0].extinction_since_tick.is_none());
    let reseed_logged = w
        .logs
        .iter()
        .any(|(s, _)| s.starts_with("✦ MYTHIC ✦ RESEED"));
    assert!(reseed_logged, "reseed mythic line should fire");
    // Reseeded C2s are alive, have C2_INITIAL_HP, and use fresh
    // faction ids beyond the original count.
    let new_c2_id = w.meshes[0].c2_nodes.last().copied().unwrap();
    assert!(matches!(w.meshes[0].nodes[new_c2_id].state, State::Alive));
    assert_eq!(w.meshes[0].nodes[new_c2_id].pwn_resist, C2_INITIAL_HP);
    assert!((w.meshes[0].nodes[new_c2_id].faction as usize) >= initial_c2_count);
    // Extinction cycle counter should have incremented.
    assert_eq!(w.extinction_cycles, 1);
}

#[test]
fn diplomacy_vassalage_derivation_is_correct_under_both_key_orderings() {
    // The canonical pair key is `(min(a,b), max(a,b))`, so for a
    // Vassalage the overlord can be either `a` (when overlord has
    // the smaller faction id) or `b` (when it has the larger).
    // `advance_relation_transitions` derives the subordinate via
    // `if overlord == a { b } else { a }` — this test pins both
    // orderings so a future refactor that breaks the derivation
    // is caught immediately.
    let cfg = Config {
        p_spawn: 0.0,
        p_loss: 0.0,
        virus_seed_rate: 0.0,
        c2_count: 3,
        epoch_period: 0,
        ..Config::default()
    };
    let mut w = World::new(1, (80, 30), cfg);
    assert_eq!(w.meshes[0].c2_nodes.len(), 3);
    // Case A: overlord = F0 (the smaller id). Canonical key = (0, 2).
    w.relations.insert(
        (0, 2),
        Relation {
            state: DiplomaticState::Vassalage { overlord: 0 },
            pressure: 0,
            trust: 0,
            entered_tick: 0,
            expires_tick: 0,
        },
    );
    // Case B: overlord = F2 (the larger id). Canonical key = (1, 2).
    w.relations.insert(
        (1, 2),
        Relation {
            state: DiplomaticState::Vassalage { overlord: 2 },
            pressure: 0,
            trust: 0,
            entered_tick: 0,
            expires_tick: 0,
        },
    );
    let f0_intel_before = w.faction_stats[0].intel;
    let f2_intel_before = w.faction_stats[2].intel;
    w.advance_diplomacy();
    // Case A: F0 is overlord, so F0 collects tribute.
    assert!(
        w.faction_stats[0].intel > f0_intel_before,
        "overlord in case A (F0, the smaller id) should collect tribute"
    );
    // Case B: F2 is overlord, so F2 collects tribute (not F1).
    assert!(
        w.faction_stats[2].intel > f2_intel_before,
        "overlord in case B (F2, the larger id) should collect tribute"
    );
    // Both relations should still exist and report their correct
    // Vassalage state after the transition pass.
    assert_eq!(
        w.relation_state(0, 2),
        DiplomaticState::Vassalage { overlord: 0 }
    );
    assert_eq!(
        w.relation_state(1, 2),
        DiplomaticState::Vassalage { overlord: 2 }
    );
}

#[test]
fn diplomacy_rebirth_produces_a_clean_relation_slate_for_the_new_faction() {
    // When a faction dies and the rebirth path spawns a new C2,
    // the new faction gets a brand-new faction id (the length
    // of c2_nodes at the time of the push). That means no
    // existing relation can reference it, and the dead faction's
    // relations are purged by the next sweep. This test walks
    // through that sequence manually.
    let cfg = Config {
        p_spawn: 0.0,
        p_loss: 0.0,
        virus_seed_rate: 0.0,
        c2_count: 2,
        epoch_period: 0,
        ..Config::default()
    };
    let mut w = World::new(1, (80, 30), cfg);
    // Plant a heavily-loaded relation between F0 and F1: active
    // Vassalage, stale trust, non-zero pressure. This is exactly
    // the kind of state that would be bad to inherit.
    w.relations.insert(
        (0, 1),
        Relation {
            state: DiplomaticState::Vassalage { overlord: 0 },
            pressure: 150,
            trust: -50,
            entered_tick: 0,
            expires_tick: 0,
        },
    );
    // Kill F1's C2 directly (assimilation-style path).
    let f1_c2 = w.meshes[0].c2_nodes[1];
    w.meshes[0].nodes[f1_c2].state = State::Dead;
    // Simulate rebirth by allocating a new faction slot — this
    // is what maybe_resurrect_c2_from_cascade does internally
    // when it picks a doomed node to promote.
    let new_faction_id = w.faction_stats.len() as u8;
    let new_branch = w.alloc_branch_id(0);
    w.meshes[0].nodes.push(Node::fresh(
        (40, 20),
        None,
        0,
        Role::Relay,
        new_branch,
    ));
    let new_c2 = w.meshes[0].nodes.len() - 1;
    w.meshes[0].nodes[new_c2].faction = new_faction_id;
    w.meshes[0].c2_nodes.push(new_c2);
    w.faction_stats.push(FactionStats::default());
    w.personas.push(Persona::Opportunist);
    w.faction_colors.push(0);
    // Run the diplomacy pass. The sweep should drop the stale
    // (0, 1) relation and the reborn faction at `new_faction_id`
    // should start with zero relations.
    w.advance_diplomacy();
    assert!(
        !w.relations.contains_key(&(0, 1)),
        "stale relation involving dead F1 should be purged"
    );
    // Reborn faction has a fresh stats entry — tier 0, no research.
    assert_eq!(w.faction_stats[new_faction_id as usize].tech_tier, 0);
    assert_eq!(w.faction_stats[new_faction_id as usize].research, 0);
    // And no relation references the new id.
    for &(a, b) in w.relations.keys() {
        assert_ne!(a, new_faction_id);
        assert_ne!(b, new_faction_id);
    }
}

#[test]
fn diplomacy_vassalage_transfers_tribute_from_vassal_to_overlord() {
    let cfg = Config {
        p_spawn: 0.0,
        p_loss: 0.0,
        virus_seed_rate: 0.0,
        c2_count: 2,
        epoch_period: 0,
        ..Config::default()
    };
    let mut w = World::new(1, (80, 30), cfg);
    // Seed a Vassalage directly. F0 is the overlord, F1 the
    // vassal. Canonical key ordering means a=0, b=1.
    w.relations.insert(
        (0, 1),
        Relation {
            state: DiplomaticState::Vassalage { overlord: 0 },
            pressure: 0,
            trust: 0,
            entered_tick: 0,
            expires_tick: 0,
        },
    );
    // Keep both factions "alive" for the sweep — they already
    // are from World::new, but make sure we haven't regressed.
    assert!(matches!(w.meshes[0].nodes[w.meshes[0].c2_nodes[0]].state, State::Alive));
    assert!(matches!(w.meshes[0].nodes[w.meshes[0].c2_nodes[1]].state, State::Alive));
    let intel_before = w.faction_stats[0].intel;
    w.advance_diplomacy();
    assert!(
        w.faction_stats[0].intel > intel_before,
        "overlord should have collected tribute from vassal"
    );
    // Relation should still exist (Vassalage has no timer).
    assert_eq!(
        w.relation_state(0, 1),
        DiplomaticState::Vassalage { overlord: 0 }
    );
}

#[test]
fn diplomacy_vassalage_rebels_when_vassal_recovers() {
    let cfg = Config {
        p_spawn: 0.0,
        p_loss: 0.0,
        virus_seed_rate: 0.0,
        c2_count: 2,
        epoch_period: 0,
        ..Config::default()
    };
    let mut w = World::new(1, (80, 30), cfg);
    // Plant enough alive nodes for the rebellion check to even
    // consider firing: overlord needs >= 8 alive, and the vassal
    // needs >= 70% of the overlord's count. Give F0 (overlord)
    // 10 alive children and F1 (vassal) 9 alive children so the
    // vassal sits at 90% of the overlord's size.
    let c2_0 = w.meshes[0].c2_nodes[0];
    let c2_1 = w.meshes[0].c2_nodes[1];
    for x in 15..25 {
        w.meshes[0].nodes.push(Node::fresh(
            (x, 10),
            Some(c2_0),
            0,
            Role::Relay,
            1,
        ));
        let last = w.meshes[0].nodes.len() - 1;
        w.meshes[0].nodes[last].faction = 0;
    }
    for x in 15..24 {
        w.meshes[0].nodes.push(Node::fresh(
            (x, 15),
            Some(c2_1),
            0,
            Role::Relay,
            1,
        ));
        let last = w.meshes[0].nodes.len() - 1;
        w.meshes[0].nodes[last].faction = 1;
    }
    w.relations.insert(
        (0, 1),
        Relation {
            state: DiplomaticState::Vassalage { overlord: 0 },
            pressure: 0,
            trust: 0,
            entered_tick: 0,
            expires_tick: 0,
        },
    );
    w.advance_diplomacy();
    // Vassal sits past 70% of overlord's size — rebellion fires,
    // Vassalage flips to ColdWar.
    assert_eq!(w.relation_state(0, 1), DiplomaticState::ColdWar);
}

#[test]
fn diplomacy_sweep_drops_relation_when_overlord_dies() {
    let cfg = Config {
        p_spawn: 0.0,
        p_loss: 0.0,
        virus_seed_rate: 0.0,
        c2_count: 2,
        epoch_period: 0,
        ..Config::default()
    };
    let mut w = World::new(1, (80, 30), cfg);
    // Plant a Vassalage where F0 is the overlord.
    w.relations.insert(
        (0, 1),
        Relation {
            state: DiplomaticState::Vassalage { overlord: 0 },
            pressure: 0,
            trust: 0,
            entered_tick: 0,
            expires_tick: 0,
        },
    );
    // Kill F0's C2 directly — this is the assimilation-style
    // path that bypasses the cascade's inline purge.
    let c2_0 = w.meshes[0].c2_nodes[0];
    w.meshes[0].nodes[c2_0].state = State::Dead;
    w.advance_diplomacy();
    assert!(
        !w.relations.contains_key(&(0, 1)),
        "sweep should drop relations whose endpoint C2 has died"
    );
}

#[test]
fn diplomacy_sweep_drops_vassalage_when_overlord_dies_but_both_endpoints_live() {
    // A rarer case: three-faction world where A and B are in
    // Vassalage with C as overlord (stored on the pair key
    // but not in the canonical pair). If C dies while A and B
    // are both alive, the sweep should still drop the relation
    // because the overlord is gone. This shouldn't normally
    // happen (overlord is always one endpoint of the canonical
    // key), but the safety sweep still handles it defensively.
    let cfg = Config {
        p_spawn: 0.0,
        p_loss: 0.0,
        virus_seed_rate: 0.0,
        c2_count: 3,
        epoch_period: 0,
        ..Config::default()
    };
    let mut w = World::new(1, (80, 30), cfg);
    assert_eq!(w.meshes[0].c2_nodes.len(), 3);
    // Plant a Vassalage with F2 as overlord, canonical key (0, 1).
    // This is an intentionally malformed state to test the
    // defensive branch — in practice Vassalage always keys one
    // of its endpoints as the overlord.
    w.relations.insert(
        (0, 1),
        Relation {
            state: DiplomaticState::Vassalage { overlord: 2 },
            pressure: 0,
            trust: 0,
            entered_tick: 0,
            expires_tick: 0,
        },
    );
    // Kill F2 (the overlord).
    let c2_2 = w.meshes[0].c2_nodes[2];
    w.meshes[0].nodes[c2_2].state = State::Dead;
    w.advance_diplomacy();
    assert!(
        !w.relations.contains_key(&(0, 1)),
        "sweep should drop Vassalage when overlord dies even if both endpoints remain alive"
    );
}

#[test]
fn diplomacy_trade_can_upgrade_through_nap_to_alliance() {
    let cfg = Config {
        p_spawn: 0.0,
        p_loss: 0.0,
        virus_seed_rate: 0.0,
        c2_count: 2,
        epoch_period: 0,
        ..Config::default()
    };
    let mut w = World::new(1, (80, 30), cfg);
    // Force both factions into Fortress so persona trust gains
    // are maxed (2.0× each side → 2.0× mult). This lets the test
    // drive an Alliance upgrade deterministically without relying
    // on RNG over thousands of ticks.
    for p in w.personas.iter_mut() {
        *p = Persona::Fortress;
    }
    // Seed a Trade state directly — the opportunistic roll in
    // advance_diplomacy is probabilistic, so we skip it for the
    // test and exercise the upgrade ladder.
    let key = (0u8, 1u8);
    // expires_tick must exceed 2 sample periods (100 ticks) so the
    // first two-loop block leaves us still in Trade, but must be low
    // enough that the trust ladder (4 per sample period with Fortress
    // × Fortress, threshold = 30) has time to accumulate ≥ 8 samples
    // before expiry. 500 ticks = 10 sample periods → trust = 40 ≥ 30.
    w.relations.insert(
        key,
        Relation {
            state: DiplomaticState::Trade,
            pressure: 0,
            trust: 0,
            entered_tick: 0,
            expires_tick: 500,
        },
    );
    // Run the machine until the Trade timer expires and
    // upgrades to NonAggression. The machine only runs inside
    // tick() on the sample cadence, so step ticks manually.
    for _ in 0..(FACTION_SAMPLE_PERIOD as usize * 2) {
        w.tick((80, 30));
    }
    // Trade → NAP requires NAP_TRUST_THRESHOLD = 30. With
    // Fortress × Fortress (2.0 mult) each Trade sample tick
    // pushes 2 * 2.0 = 4 trust, so 30/4 ≈ 8 sample periods. In
    // the loop we have 2 sample periods, so we'll be in Trade
    // still. We advance more to cover the full ladder.
    for _ in 0..(FACTION_SAMPLE_PERIOD as usize * 40) {
        w.tick((80, 30));
    }
    // By now the pair should be in either NonAggression or
    // Alliance depending on how many ladder steps happened.
    let state = w.relation_state(0, 1);
    assert!(
        matches!(state, DiplomaticState::NonAggression | DiplomaticState::Alliance),
        "expected NAP or Alliance, got {:?}",
        state
    );
    // And peace helpers should agree.
    assert!(w.allied(0, 1));
    assert!(!w.at_war(0, 1));
}

#[test]
fn era_transition_rebinds_active_rules() {
    // Short epoch so we can cross several boundaries quickly, and
    // kill all background rolls so the tick loop's only notable work
    // is the era transition bookkeeping.
    let cfg = Config {
        p_spawn: 0.0,
        p_loss: 0.0,
        virus_seed_rate: 0.0,
        worm_spawn_rate: 0.0,
        reconnect_rate: 0.0,
        epoch_period: 10,
        ..Config::default()
    };
    let mut w = World::new(1, (80, 30), cfg);
    // Opening era is "Age of Silence" — packets hushed, losses eased.
    assert_eq!(w.epoch_index(), 0);
    assert!((w.era_rules.exfil_period_mult - 2.0).abs() < 1e-6);
    assert!((w.era_rules.loss_mult - 0.7).abs() < 1e-6);
    // Cross into era 1 ("First Signal") — spawn surge, nothing else.
    // The epoch-boundary check runs inside `tick()` before the
    // `self.tick += 1` at the end, so reaching tick=10 requires 11
    // calls (first call processes tick=0 and increments to 1).
    for _ in 0..11 {
        w.tick((80, 30));
    }
    assert_eq!(w.epoch_index(), 1);
    assert!((w.era_rules.spawn_mult - 1.3).abs() < 1e-6);
    assert!((w.era_rules.loss_mult - 1.0).abs() < 1e-6);
    // And again across tick=20 and tick=30 into era 3 ("Era of Cascades").
    for _ in 0..20 {
        w.tick((80, 30));
    }
    assert_eq!(w.epoch_index(), 3);
    assert!((w.era_rules.cascade_mult - 2.0).abs() < 1e-6);
    // A log line fired for each transition with the summary suffix.
    let era_lines = w
        .logs
        .iter()
        .filter(|(s, _)| s.starts_with("✦ era"))
        .count();
    assert!(era_lines >= 3, "expected ≥3 era log lines, got {}", era_lines);
}

#[test]
fn c2_count_randomized_within_range() {
    // With c2_count=2 and c2_count_max=4, every seed should land
    // somewhere in 2..=4, but different seeds hit different counts.
    let mut counts = std::collections::HashSet::new();
    for seed in 0..50u64 {
        let cfg = Config {
            c2_count: 2,
            c2_count_max: 4,
            p_spawn: 0.0,
            ..Config::default()
        };
        let w = World::new(seed, (120, 30), cfg);
        let n = w.meshes[0].c2_nodes.len();
        assert!((2..=4).contains(&n), "seed {} gave {} c2s", seed, n);
        counts.insert(n);
    }
    // With 50 seeds, we should see at least 2 distinct counts.
    assert!(counts.len() >= 2, "expected varied counts, got {:?}", counts);
}

#[test]
fn large_cascade_can_resurrect_a_new_c2() {
    let cfg = Config {
        p_spawn: 0.0,
        p_loss: 0.0,
        virus_seed_rate: 0.0,
        resurrection_threshold: 3,
        resurrection_chance: 1.0,
        ..Config::default()
    };
    let mut w = World::new(90, (80, 30), cfg);
    // Build a short chain c2 -> a -> b -> c, where c is the
    // cascade root. schedule_subtree_death(c) should doom c and
    // roll the resurrection (threshold=3, chance=1.0 = guaranteed).
    let a = w.meshes[0].nodes.len();
    w.meshes[0].nodes
        .push(Node::fresh((10, 10), Some(w.primary_c2), 0, Role::Relay, 1));
    let b = w.meshes[0].nodes.len();
    w.meshes[0].nodes.push(Node::fresh((11, 10), Some(a), 0, Role::Relay, 1));
    let c = w.meshes[0].nodes.len();
    w.meshes[0].nodes.push(Node::fresh((12, 10), Some(b), 0, Role::Relay, 1));
    // Full parent-link chain so compute_cascade can walk it.
    let make_path = |x0: i16, x1: i16| -> Vec<(i16, i16)> {
        (x0..=x1).map(|x| (x, 10)).collect()
    };
    let push_link = |w: &mut World, a: usize, b: usize, x0: i16, x1: i16| {
        let path = make_path(x0, x1);
        let len = path.len() as u16;
        w.meshes[0].links.push(Link {
            a,
            b,
            path,
            drawn: len,
            kind: LinkKind::Parent,
            load: 0,
            breach_ttl: 0,
            burn_ticks: 0,
            quarantined: 0,
            packets_delivered: 0,
            is_backbone: false,
        black_market_until: 0,
            latent: false,
        });
    };
    let c2 = w.primary_c2;
    push_link(&mut w, c2, a, 0, 10);
    push_link(&mut w, a, b, 10, 11);
    push_link(&mut w, b, c, 11, 12);

    let before = w.meshes[0].c2_nodes.len();
    // Schedule the whole subtree rooted at `a` (a, b, c → 3 nodes).
    w.schedule_subtree_death(0, a, 1.0);
    let after = w.meshes[0].c2_nodes.len();
    assert_eq!(
        after,
        before + 1,
        "expected a rebirth to add one C2; before={} after={}",
        before,
        after
    );
    // The resurrected C2 should be parentless and alive.
    let new_c2 = *w.meshes[0].c2_nodes.last().unwrap();
    assert!(matches!(w.meshes[0].nodes[new_c2].state, State::Alive));
    assert!(w.meshes[0].nodes[new_c2].parent.is_none());
}

#[test]
fn tick_runs_without_panic_and_grows() {
    let mut w = World::new(42, (80, 24), Config::default());
    for _ in 0..500 {
        w.tick((80, 24));
    }
    assert!(w.meshes[0].nodes.len() > 1);
}
