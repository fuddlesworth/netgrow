use super::*;

#[test]
fn scheduled_subtree_death_eventually_kills_all_descendants() {
    let mut w = World::new(1, (80, 30), Config::default());
    // Kill the RNG-driven loss/spawn so only our scheduled death runs.
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    // Manually build a 3-level tree: c2 -> a -> b -> c
    let a = w.nodes.len();
    w.nodes
        .push(Node::fresh((10, 10), Some(w.c2()), 0, Role::Relay, 1));
    let b = w.nodes.len();
    w.nodes.push(Node::fresh((12, 10), Some(a), 0, Role::Relay, 1));
    let c = w.nodes.len();
    w.nodes.push(Node::fresh((14, 10), Some(b), 0, Role::Relay, 1));
    w.schedule_subtree_death(a, 1.0);
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
    assert!(matches!(w.nodes[w.c2()].state, State::Alive));
}

#[test]
fn hardened_node_resists_first_pwn() {
    let mut w = World::new(7, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    let id = w.nodes.len();
    let mut n = Node::fresh((10, 10), Some(w.c2()), 0, Role::Relay, 1);
    n.hardened = true;
    w.nodes.push(n);
    w.cfg.p_loss = 1.0; // force the victim roll to fire
    w.advance_pwned_and_loss();
    assert!(matches!(w.nodes[id].state, State::Alive));
    assert!(!w.nodes[id].hardened);
}

#[test]
fn branch_id_inherits_from_parent_not_c2() {
    let mut w = World::new(11, (120, 40), Config::default());
    // First-hop child gets fresh branch id.
    let a = w.alloc_branch_id();
    w.nodes
        .push(Node::fresh((30, 10), Some(w.c2()), 0, Role::Relay, a));
    let a_id = w.nodes.len() - 1;
    w.nodes
        .push(Node::fresh((32, 10), Some(a_id), 0, Role::Relay, w.nodes[a_id].branch_id));
    assert_ne!(w.nodes[a_id].branch_id, 0);
    assert_eq!(w.nodes[a_id + 1].branch_id, w.nodes[a_id].branch_id);
}

#[test]
fn packet_reaches_c2_and_drops() {
    let mut w = World::new(3, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    // Build chain c2 -> a -> b (exfil)
    let a = w.nodes.len();
    w.nodes
        .push(Node::fresh((10, 10), Some(w.c2()), 0, Role::Relay, 1));
    let b = w.nodes.len();
    w.nodes
        .push(Node::fresh((14, 10), Some(a), 0, Role::Exfil, 1));
    // Manufacture links with full paths marked drawn.
    let path_ca: Vec<(i16, i16)> =
        (w.nodes[w.c2()].pos.0..=10).map(|x| (x, 10)).collect();
    let len_ca = path_ca.len() as u16;
    w.links.push(Link {
        a: w.c2(),
        b: a,
        path: path_ca,
        drawn: len_ca,
        kind: LinkKind::Parent,
        load: 0,
        breach_ttl: 0,
        burn_ticks: 0,
        quarantined: 0,
    });
    let path_ab: Vec<(i16, i16)> = (10..=14).map(|x| (x, 10)).collect();
    let len_ab = path_ab.len() as u16;
    w.links.push(Link {
        a,
        b,
        path: path_ab,
        drawn: len_ab,
        kind: LinkKind::Parent,
        load: 0,
        breach_ttl: 0,
        burn_ticks: 0,
        quarantined: 0,
    });
    // Force the Exfil to fire on tick 0 and then tick enough for the
    // packet to reach C2 and be dropped.
    w.nodes[b].role_cooldown = 0;
    w.fire_exfil_packets();
    assert_eq!(w.packets.len(), 1);
    for _ in 0..40 {
        w.advance_packets();
    }
    assert!(w.packets.is_empty());
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
    let a = w.nodes.len();
    w.nodes
        .push(Node::fresh((20, 10), Some(w.c2()), 0, Role::Relay, 1));
    let c = w.nodes.len();
    w.nodes
        .push(Node::fresh((30, 10), Some(w.c2()), 0, Role::Relay, 2));
    let b = w.nodes.len();
    w.nodes.push(Node::fresh((25, 12), Some(a), 0, Role::Relay, 1));
    // Fully-drawn cross link b ↔ c.
    let cross_path = vec![(25, 12), (30, 10)]; // cells don't matter for logic
    let len = cross_path.len() as u16;
    w.links.push(Link {
        a: b,
        b: c,
        path: cross_path,
        drawn: len,
        kind: LinkKind::Cross,
        load: 0,
        breach_ttl: 0,
        burn_ticks: 0,
        quarantined: 0,
    });
    let cascade = w.compute_cascade(a);
    let ids: HashSet<NodeId> = cascade.iter().map(|(id, _)| *id).collect();
    assert!(ids.contains(&a), "root must be doomed");
    assert!(!ids.contains(&b), "b should survive via cross link to c");
    assert!(!ids.contains(&c), "c has its own route to C2");
}

#[test]
fn shield_flash_is_set_when_hardened_node_is_hit() {
    let mut w = World::new(9, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 1.0;
    let id = w.nodes.len();
    let mut n = Node::fresh((10, 10), Some(w.c2()), 0, Role::Relay, 1);
    n.hardened = true;
    w.nodes.push(n);
    w.advance_pwned_and_loss();
    assert!(matches!(w.nodes[id].state, State::Alive));
    assert!(!w.nodes[id].hardened);
    assert!(w.nodes[id].shield_flash > 0, "shield flash should be set");
    // The flash should drain over subsequent ticks.
    w.cfg.p_loss = 0.0; // don't hit it again
    for _ in 0..10 {
        w.tick((80, 30));
    }
    assert_eq!(w.nodes[id].shield_flash, 0);
}

#[test]
fn reconnect_creates_cross_link_between_branches() {
    let mut w = World::new(13, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    w.cfg.reconnect_rate = 1.0;
    w.cfg.reconnect_radius = 20;
    // Two alive nodes in different branches, no existing bridge.
    w.nodes
        .push(Node::fresh((20, 10), Some(w.c2()), 0, Role::Relay, 1));
    w.nodes
        .push(Node::fresh((25, 12), Some(w.c2()), 0, Role::Relay, 2));
    let before = w.links.iter().filter(|l| l.kind == LinkKind::Cross).count();
    w.maybe_reconnect();
    let after = w.links.iter().filter(|l| l.kind == LinkKind::Cross).count();
    assert_eq!(after, before + 1, "should have formed exactly one cross link");
    // Second call should not create a duplicate between the same pair.
    w.maybe_reconnect();
    let cross_count = w.links.iter().filter(|l| l.kind == LinkKind::Cross).count();
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
    w.nodes
        .push(Node::fresh((20, 10), Some(w.c2()), 0, Role::Relay, 1));
    w.nodes
        .push(Node::fresh((25, 12), Some(w.c2()), 0, Role::Relay, 1));
    for _ in 0..20 {
        w.maybe_reconnect();
    }
    let cross = w.links.iter().filter(|l| l.kind == LinkKind::Cross).count();
    assert_eq!(cross, 0);
}

#[test]
fn infection_spreads_along_parent_edges() {
    let mut w = World::new(21, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    w.cfg.virus_seed_rate = 0.0;
    w.cfg.virus_spread_rate = 1.0;
    // Build c2 -> a -> b, infect a and drive it straight to Active so it
    // can infect neighbors.
    let a = w.nodes.len();
    w.nodes
        .push(Node::fresh((10, 10), Some(w.c2()), 0, Role::Relay, 1));
    let b = w.nodes.len();
    w.nodes.push(Node::fresh((12, 10), Some(a), 0, Role::Relay, 1));
    w.nodes[a].infection = Some(Infection {
        strain: 3,
        stage: InfectionStage::Active,
        age: w.cfg.virus_incubation_ticks,
        cure_resist: 3,
        terminal_ticks: 0,
        is_ransom: false,
    });
    // Run a few ticks: spread probability is 1.0 so b should catch it fast.
    for _ in 0..5 {
        w.tick((80, 30));
    }
    assert!(w.nodes[b].infection.is_some());
    assert_eq!(w.nodes[b].infection.unwrap().strain, 3);
}

#[test]
fn infection_skips_c2() {
    let mut w = World::new(22, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    w.cfg.virus_seed_rate = 0.0;
    w.cfg.virus_spread_rate = 1.0;
    // Child directly attached to C2, infected and Active.
    let a = w.nodes.len();
    w.nodes
        .push(Node::fresh((10, 10), Some(w.c2()), 0, Role::Relay, 1));
    w.nodes[a].infection = Some(Infection {
        strain: 0,
        stage: InfectionStage::Active,
        age: w.cfg.virus_incubation_ticks,
        cure_resist: 3,
        terminal_ticks: 0,
        is_ransom: false,
    });
    for _ in 0..20 {
        w.tick((80, 30));
    }
    assert!(w.nodes[w.c2()].infection.is_none(), "C2 must stay clean");
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
    let c2_pos = w.nodes[w.c2()].pos;
    let a = w.nodes.len();
    w.nodes.push(Node::fresh(
        (c2_pos.0 + 3, c2_pos.1),
        Some(w.c2()),
        0,
        Role::Relay,
        1,
    ));
    w.nodes[a].infection = Some(Infection {
        strain: 0,
        stage: InfectionStage::Incubating,
        age: 0,
        cure_resist: 1,
        terminal_ticks: 0,
        is_ransom: false,
    });
    // Seed a patch wave directly and tick it forward until the front hits.
    w.patch_waves.push(PatchWave {
        origin: c2_pos,
        radius: 0,
    });
    for _ in 0..10 {
        w.advance_patch_waves();
        if w.nodes[a].infection.is_none() {
            break;
        }
    }
    assert!(w.nodes[a].infection.is_none(), "patch wave should cure the node");
}

#[test]
fn worm_delivered_to_alive_neighbor() {
    let mut w = World::new(25, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    w.cfg.virus_seed_rate = 0.0;
    w.cfg.virus_spread_rate = 0.0;
    // Build c2 -> a -> b with fully-drawn links.
    let a = w.nodes.len();
    w.nodes
        .push(Node::fresh((10, 10), Some(w.c2()), 0, Role::Relay, 1));
    let b = w.nodes.len();
    w.nodes.push(Node::fresh((14, 10), Some(a), 0, Role::Relay, 1));
    let path_ab: Vec<(i16, i16)> = (10..=14).map(|x| (x, 10)).collect();
    let len_ab = path_ab.len() as u16;
    w.links.push(Link {
        a,
        b,
        path: path_ab,
        drawn: len_ab,
        kind: LinkKind::Parent,
        load: 0,
        breach_ttl: 0,
        burn_ticks: 0,
        quarantined: 0,
    });
    // Launch a worm from a → b manually and tick the worm advance step
    // enough times for it to reach the far end.
    w.worms.push(Worm {
        link_id: 0,
        pos: 0,
        outbound_from_a: true,
        strain: 2,
    });
    for _ in 0..10 {
        w.advance_worms();
    }
    assert!(w.nodes[b].infection.is_some());
    assert_eq!(w.nodes[b].infection.unwrap().strain, 2);
    assert!(w.worms.is_empty());
}

#[test]
fn terminal_infection_forces_loss() {
    let mut w = World::new(23, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    w.cfg.virus_seed_rate = 0.0;
    w.cfg.virus_spread_rate = 0.0;
    let a = w.nodes.len();
    w.nodes
        .push(Node::fresh((10, 10), Some(w.c2()), 0, Role::Relay, 1));
    w.nodes[a].infection = Some(Infection {
        strain: 0,
        stage: InfectionStage::Terminal,
        age: 200,
        cure_resist: 3,
        terminal_ticks: 1,
        is_ransom: false,
    });
    // One tick drains terminal_ticks and flips to Pwned.
    w.tick((80, 30));
    assert!(matches!(
        w.nodes[a].state,
        State::Pwned { .. } | State::Dead
    ));
    assert!(w.nodes[a].infection.is_none());
}

#[test]
fn mutation_skips_honeypots() {
    let mut w = World::new(26, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    w.cfg.mutate_rate = 1.0;
    w.cfg.mutate_min_age = 0;
    w.cfg.virus_seed_rate = 0.0;
    let id = w.nodes.len();
    w.nodes
        .push(Node::fresh((10, 10), Some(w.c2()), 0, Role::Honeypot, 1));
    for _ in 0..10 {
        w.maybe_mutate();
    }
    assert_eq!(w.nodes[id].role, Role::Honeypot);
    assert_eq!(w.nodes[id].mutated_flash, 0);
}

#[test]
fn mutation_flips_relay_role_and_flashes() {
    let mut w = World::new(27, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    w.cfg.mutate_rate = 1.0;
    w.cfg.mutate_min_age = 0;
    w.cfg.virus_seed_rate = 0.0;
    let id = w.nodes.len();
    w.nodes
        .push(Node::fresh((10, 10), Some(w.c2()), 0, Role::Relay, 1));
    w.maybe_mutate();
    assert!(matches!(w.nodes[id].role, Role::Scanner | Role::Exfil));
    assert!(w.nodes[id].mutated_flash > 0);
}

#[test]
fn zero_day_respects_min_node_floor() {
    let mut w = World::new(28, (80, 30), Config::default());
    w.cfg.zero_day_period = 1;
    w.cfg.zero_day_chance = 1.0;
    w.cfg.virus_seed_rate = 0.0;
    // Only C2 alive: 1 node, well below the 10-node minimum.
    w.tick = 1;
    w.maybe_zero_day();
    assert!(w.nodes.iter().all(|n| n.infection.is_none()));
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
        w.nodes
            .push(Node::fresh((10 + i, 10), Some(w.c2()), 0, Role::Relay, 1));
    }
    w.zero_day_outbreak();
    let infected = w
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
    let infected = w.nodes.len();
    w.nodes
        .push(Node::fresh((10, 10), Some(w.c2()), 0, Role::Relay, 1));
    let honey = w.nodes.len();
    w.nodes
        .push(Node::fresh((12, 10), Some(infected), 0, Role::Honeypot, 1));
    w.nodes[infected].infection = Some(Infection {
        strain: 0,
        stage: InfectionStage::Active,
        age: w.cfg.virus_incubation_ticks,
        cure_resist: 4,
        terminal_ticks: 0,
        is_ransom: false,
    });
    for _ in 0..20 {
        w.tick((80, 30));
    }
    assert!(w.nodes[honey].infection.is_none(), "honeypot must stay clean");
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
    let _defender = w.nodes.len();
    w.nodes
        .push(Node::fresh((10, 10), Some(w.c2()), 0, Role::Defender, 1));
    let victim = w.nodes.len();
    w.nodes
        .push(Node::fresh((12, 11), Some(w.c2()), 0, Role::Relay, 1));
    w.nodes[victim].infection = Some(Infection {
        strain: 0,
        stage: InfectionStage::Active,
        age: w.cfg.virus_incubation_ticks,
        cure_resist: 1,
        terminal_ticks: 0,
        is_ransom: false,
    });
    w.fire_defender_pulses();
    assert!(w.nodes[victim].infection.is_none(), "defender should clear infection in radius");
}

#[test]
fn defender_immune_to_infection() {
    let mut w = World::new(34, (80, 30), Config::default());
    w.cfg.p_spawn = 0.0;
    w.cfg.p_loss = 0.0;
    w.cfg.virus_seed_rate = 0.0;
    w.cfg.virus_spread_rate = 1.0;
    let infected = w.nodes.len();
    w.nodes
        .push(Node::fresh((10, 10), Some(w.c2()), 0, Role::Relay, 1));
    let defender = w.nodes.len();
    w.nodes
        .push(Node::fresh((12, 10), Some(infected), 0, Role::Defender, 1));
    w.nodes[infected].infection = Some(Infection {
        strain: 0,
        stage: InfectionStage::Active,
        age: w.cfg.virus_incubation_ticks,
        cure_resist: 4,
        terminal_ticks: 0,
        is_ransom: false,
    });
    for _ in 0..20 {
        w.tick((80, 30));
    }
    assert!(w.nodes[defender].infection.is_none(), "defender should never get infected");
}

#[test]
fn multiple_c2s_each_get_distinct_factions() {
    let cfg = Config {
        c2_count: 3,
        p_spawn: 0.0,
        ..Config::default()
    };
    let w = World::new(40, (120, 30), cfg);
    assert_eq!(w.c2_nodes.len(), 3);
    assert_eq!(w.nodes[w.c2_nodes[0]].faction, 0);
    assert_eq!(w.nodes[w.c2_nodes[1]].faction, 1);
    assert_eq!(w.nodes[w.c2_nodes[2]].faction, 2);
    // Random placement with minimum spacing — every pair must be
    // at a distinct cell.
    for i in 0..w.c2_nodes.len() {
        for j in (i + 1)..w.c2_nodes.len() {
            assert_ne!(
                w.nodes[w.c2_nodes[i]].pos,
                w.nodes[w.c2_nodes[j]].pos,
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
    let f0 = w.c2_nodes[0];
    let f1 = w.c2_nodes[1];
    let child0 = w.nodes.len();
    let mut n0 = Node::fresh((10, 10), Some(f0), 0, Role::Relay, 1);
    n0.faction = 0;
    w.nodes.push(n0);
    let child1 = w.nodes.len();
    let mut n1 = Node::fresh((40, 10), Some(f1), 0, Role::Relay, 2);
    n1.faction = 1;
    w.nodes.push(n1);
    // Trigger a cascade on faction 0's child. Faction 1 must survive.
    w.schedule_subtree_death(child0, 1.0);
    for _ in 0..20 {
        w.tick((80, 30));
    }
    assert!(matches!(w.nodes[child0].state, State::Dead));
    assert!(matches!(w.nodes[child1].state, State::Alive));
    assert!(matches!(w.nodes[f1].state, State::Alive));
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
    let a = w.nodes.len();
    w.nodes
        .push(Node::fresh((10, 10), Some(w.c2()), 0, Role::Relay, 1));
    let b = w.nodes.len();
    w.nodes.push(Node::fresh((14, 10), Some(a), 0, Role::Exfil, 1));
    let path: Vec<(i16, i16)> = (10..=14).map(|x| (x, 10)).collect();
    let len = path.len() as u16;
    w.links.push(Link {
        a,
        b,
        path,
        drawn: len,
        kind: LinkKind::Parent,
        load: 0,
        breach_ttl: 0,
        burn_ticks: 0,
        quarantined: 0,
    });
    // Park a packet on the link and tick the motion phase a few times.
    w.packets.push(Packet {
        link_id: 0,
        pos: len - 1,
    });
    for _ in 0..5 {
        w.decay_link_load();
        w.advance_packets();
    }
    assert!(w.links[0].load > 0, "load should accumulate from in-flight packet");

    // Stop feeding packets; load decays back to zero.
    w.packets.clear();
    for _ in 0..20 {
        w.decay_link_load();
    }
    assert_eq!(w.links[0].load, 0, "load should decay to zero");
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
    let honey = w.nodes.len();
    let mut h = Node::fresh((20, 10), Some(w.c2()), 0, Role::Honeypot, 1);
    h.faction = 0;
    w.nodes.push(h);
    for (i, pos) in [(25, 10), (18, 15), (22, 12)].iter().enumerate() {
        let mut n = Node::fresh(*pos, Some(w.c2()), 0, Role::Relay, 2 + i as u16);
        n.faction = 0;
        w.nodes.push(n);
    }
    let before = w
        .links
        .iter()
        .filter(|l| l.kind == LinkKind::Cross)
        .count();
    w.reveal_honeypot_backdoors(honey);
    let after = w
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
    // One tower adjacent to C2 — the only possible victim.
    let tower = w.nodes.len();
    let mut n = Node::fresh((11, 10), Some(w.c2()), 0, Role::Tower, 1);
    n.pwn_resist = 2;
    n.faction = 0;
    w.nodes.push(n);
    // Three ticks of guaranteed loss rolls. First two should consume
    // the pwn_resist charges; third should finally pwn the tower.
    for _ in 0..2 {
        w.tick((80, 30));
        assert!(
            matches!(w.nodes[tower].state, State::Alive),
            "tower should still be alive"
        );
    }
    w.tick((80, 30));
    assert!(
        matches!(w.nodes[tower].state, State::Pwned { .. } | State::Dead),
        "tower should be down after 3 hits"
    );
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
        let n = w.c2_nodes.len();
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
    // Manually stage three Alive nodes with dying_in == 1 so they
    // all die on the next advance_dying tick, triggering the
    // resurrection roll (threshold=3, chance=1.0 = guaranteed).
    for i in 0..3 {
        let id = w.nodes.len();
        let mut n = Node::fresh((10 + i, 10), Some(w.c2()), 0, Role::Relay, 1);
        n.faction = 0;
        n.dying_in = 1;
        w.nodes.push(n);
        let _ = id;
    }
    let before = w.c2_nodes.len();
    w.advance_dying();
    let after = w.c2_nodes.len();
    assert_eq!(
        after,
        before + 1,
        "expected a rebirth to add one C2; before={} after={}",
        before,
        after
    );
    // The resurrected C2 should be parentless and alive.
    let new_c2 = *w.c2_nodes.last().unwrap();
    assert!(matches!(w.nodes[new_c2].state, State::Alive));
    assert!(w.nodes[new_c2].parent.is_none());
}

#[test]
fn tick_runs_without_panic_and_grows() {
    let mut w = World::new(42, (80, 24), Config::default());
    for _ in 0..500 {
        w.tick((80, 24));
    }
    assert!(w.nodes.len() > 1);
}
