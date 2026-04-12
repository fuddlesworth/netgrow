//! World runtime configuration and static name pools.
//!
//! Holds the tunable `Config` struct that drives spawn/loss rates,
//! periodic events, virus tuning, faction behavior, and render cadence,
//! plus the flavor-text pools (`STRAIN_NAME_POOL`, `ERA_NAMES`) used to
//! give each run narrative color. Isolated from `world/mod.rs` so the
//! state machine stays focused on tick logic.

use super::RoleWeights;

#[derive(Clone, Debug)]
pub struct Config {
    pub p_spawn: f32,
    pub p_loss: f32,
    pub base_dist: i16,
    pub max_nodes: usize,
    pub heartbeat_period: u64,
    pub pwned_flash_ticks: u8,
    pub log_cap: usize,
    pub role_weights: RoleWeights,
    pub scanner_ping_period: u16,
    pub exfil_packet_period: u16,
    pub hardened_after_heartbeats: u8,
    pub honeypot_cascade_mult: f32,
    pub reconnect_rate: f32,
    pub reconnect_radius: i16,
    pub virus_spread_rate: f32,
    pub virus_incubation_ticks: u16,
    pub virus_active_ticks: u16,
    pub virus_terminal_ticks: u8,
    pub virus_cure_resist: u8,
    pub virus_seed_rate: f32,
    pub worm_spawn_rate: f32,
    pub patch_wave_radius: i16,
    pub mutate_rate: f32,
    pub mutate_min_age: u64,
    pub zero_day_period: u64,
    pub zero_day_chance: f32,
    /// Constant weight given to C2 in the parent-selection roll. Without
    /// this, C2's age-decayed weight collapses below all frontier nodes
    /// after the first few ticks and the mesh stops minting new branches.
    pub c2_spawn_bias: f32,
    /// Per-spawn probability that a new node starts its own branch instead
    /// of inheriting its parent's branch_id. Lets distinct sub-botnets fork
    /// off existing nodes anywhere in the mesh, not just at C2.
    pub fork_rate: f32,
    /// Ticks between defender cure pulses.
    pub defender_pulse_period: u16,
    /// Chebyshev radius of a defender's local cure pulse.
    pub defender_radius: i16,
    /// Number of C2 nodes / factions to spawn at the start of the run.
    /// 1 = classic single botnet; 2+ = competing factions.
    pub c2_count: u8,
    /// Length of a full day/night cycle in ticks. Spawn and loss rates
    /// oscillate across this period, creating visible waves of activity.
    /// 0 disables the effect entirely.
    pub day_night_period: u64,
    /// Multiplier applied to p_spawn during the night half of the cycle.
    pub night_spawn_mult: f32,
    /// Multiplier applied to p_loss during the night half of the cycle.
    pub night_loss_mult: f32,
    /// Chebyshev radius searched for honeypot backdoor targets. When a
    /// honeypot trips, it reveals up to `honeypot_backdoor_max` new
    /// cross-links to nearby same-faction / different-branch neighbors
    /// before cascading.
    pub honeypot_backdoor_radius: i16,
    /// Maximum number of backdoor cross-links a single honeypot trip
    /// may reveal.
    pub honeypot_backdoor_max: u8,
    /// Ticks between network-storm rolls. A storm is a rare event that
    /// briefly spikes both spawn and loss, producing a chaotic burst.
    /// Set to 0 to disable.
    pub storm_period: u64,
    /// Probability of a storm firing when `storm_period` elapses.
    pub storm_chance: f32,
    /// How many ticks a storm stays active once it fires.
    pub storm_duration: u64,
    /// Multiplier applied to p_spawn while a storm is active.
    pub storm_spawn_mult: f32,
    /// Multiplier applied to p_loss while a storm is active.
    pub storm_loss_mult: f32,
    /// Ticks between DDoS wave rolls. A DDoS wave sweeps across the
    /// mesh from a random edge to the opposite one, temporarily
    /// spiking role cooldowns on any node it passes over.
    pub ddos_period: u64,
    /// Probability a DDoS wave fires when `ddos_period` elapses.
    pub ddos_chance: f32,
    /// Number of ticks added to role_cooldown on any node the wave
    /// sweeps across.
    pub ddos_stun_ticks: u16,
    /// Ticks between wormhole spawn rolls.
    pub wormhole_period: u64,
    pub wormhole_chance: f32,
    pub wormhole_life_ticks: u16,
    /// Ticks between ISP outage rolls. A successful roll spawns a
    /// rectangular dead zone somewhere on the mesh that blocks new
    /// spawns and stuns alive nodes inside it.
    pub isp_outage_period: u64,
    pub isp_outage_chance: f32,
    pub isp_outage_life_ticks: u16,
    /// Side length range (Chebyshev) of an ISP outage rectangle.
    pub isp_outage_min_side: i16,
    pub isp_outage_max_side: i16,
    /// Ticks between network partition rolls. A partition is a
    /// horizontal or vertical slice through the mesh that drops
    /// packets/worms trying to cross it and blocks new cross-
    /// faction bridges through the cut.
    pub partition_period: u64,
    pub partition_chance: f32,
    pub partition_life_ticks: u16,
    /// Per-spawn chance that a freshly minted node is secretly a
    /// sleeper agent loyal to a different faction. Stays dormant
    /// until `maybe_wake_sleepers` rolls a wake.
    pub sleeper_spawn_chance: f32,
    /// Tick period between sleeper wake rolls. Each active sleeper
    /// rolls once per period at `sleeper_wake_chance`.
    pub sleeper_wake_period: u64,
    pub sleeper_wake_chance: f32,
    /// Maximum Chebyshev distance from any C2 at which a Tower may
    /// spawn. Spawns rolling a Tower role beyond this distance fall
    /// back to Relay, so fortified cores stay near their faction hub.
    pub tower_spawn_radius: i16,
    /// Extra pwn-absorbing charges a newly spawned Tower receives.
    pub tower_pwn_resist: u8,
    /// Chance that a newly seeded or injected infection is a ransomware
    /// variant — immune to patch waves, only cleared by defender pulses.
    pub ransom_chance: f32,
    /// Chance that a newly seeded infection is a carrier variant —
    /// endemic, never terminal, keeps re-infecting its neighbors.
    pub carrier_chance: f32,
    /// Ticks between bandwidth-drought rolls. During an active
    /// drought, the effective HOT_LINK / BACKBONE_HOT_LINK
    /// ceiling drops by `drought_hot_penalty`, so packet routing
    /// refuses moderately-loaded links and exfils throttle
    /// sooner. Set to 0 to disable.
    pub drought_period: u64,
    pub drought_chance: f32,
    pub drought_duration: u64,
    /// Amount subtracted from the hot-link ceiling while a
    /// drought is active. Higher values produce more brutal
    /// traffic prioritization.
    pub drought_hot_penalty: u8,
    /// Ticks between defector event rolls. A defection picks a
    /// non-C2 alive node, flips it to a rival faction, reparents
    /// it to the nearest recipient node, and credits the
    /// receiving faction with `defector_intel_reward` intel as
    /// "topology memory carried across the lines." Set to 0 to
    /// disable.
    pub defector_period: u64,
    pub defector_chance: f32,
    /// Intel bump awarded to the receiving faction when a
    /// defector lands, modeling the partial topology knowledge
    /// they brought with them.
    pub defector_intel_reward: u32,
    /// Chance that a reconnect pick may bridge two DIFFERENT factions
    /// instead of the default same-faction-only rule. When a cross-
    /// faction bridge forms, worms can travel between factions,
    /// enabling viral warfare.
    pub cross_faction_bridge_chance: f32,
    /// Ticks between assimilation checks.
    pub assimilation_period: u64,
    /// Below this many alive nodes, a faction becomes a candidate for
    /// assimilation.
    pub assimilation_threshold: usize,
    /// A candidate faction is absorbed only when another faction has
    /// at least this many alive nodes.
    pub assimilation_dominance: usize,
    /// Ticks between border-skirmish checks. A skirmish resolves
    /// p_loss-style attacks at faction frontiers, so touching enemy
    /// territory is costly.
    pub border_skirmish_period: u64,
    /// Chebyshev radius considered "at the border" for skirmishes.
    pub border_skirmish_radius: i16,
    /// Per-border-node chance of losing shielding / taking a hit on
    /// each skirmish tick.
    pub border_skirmish_chance: f32,
    /// Length of a single named epoch in ticks. Each time the sim crosses
    /// a multiple of this value, it enters a new era with a name drawn
    /// from ERA_NAMES. Set to 0 to disable.
    pub epoch_period: u64,
    /// Radius within which a Proxy node echoes a firing scanner's
    /// pulse. When a scanner fires, every proxy inside this Chebyshev
    /// radius also gets scan_pulse set, so the pulse ripples through
    /// a chain of proxies.
    pub proxy_radius: i16,
    /// Radius within which an alive Beacon boosts a candidate's
    /// parent-selection weight during try_spawn.
    pub beacon_radius: i16,
    /// Multiplier added to a candidate's parent weight per nearby beacon.
    /// Default 1.5x means being next to a beacon roughly 2.5x a node's
    /// chance of being chosen to spawn the next child.
    pub beacon_weight_mult: f32,
    /// A cascade of this size or larger logs 'THE BIG ONE' as a mythic
    /// event. Tune lower for a smaller mesh or to see it more often.
    pub mythic_big_one_threshold: usize,
    /// If greater than `c2_count`, World::new rolls a random starting
    /// C2 count in `c2_count..=c2_count_max` instead of the fixed
    /// value. Lets each seed produce a differently-shaped opening.
    pub c2_count_max: u8,
    /// Minimum size of a cascade batch that can trigger a rebirth
    /// roll. Below this, cascades never resurrect anything.
    pub resurrection_threshold: u8,
    /// Chance that a qualifying cascade batch births a new C2 from
    /// one of its ashes.
    pub resurrection_chance: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            p_spawn: 0.15,
            p_loss: 0.005,
            base_dist: 4,
            max_nodes: 400,
            heartbeat_period: 20,
            pwned_flash_ticks: 18,
            log_cap: 32,
            role_weights: RoleWeights::default(),
            scanner_ping_period: 30,
            exfil_packet_period: 18,
            hardened_after_heartbeats: 10,
            honeypot_cascade_mult: 3.0,
            reconnect_rate: 0.015,
            reconnect_radius: 10,
            virus_spread_rate: 0.05,
            virus_incubation_ticks: 30,
            virus_active_ticks: 80,
            virus_terminal_ticks: 20,
            // With the width-1 patch wave (post-bugfix) each wave decrements
            // cure_resist exactly once per pass. Set to 2 so infections clear
            // after two heartbeat sweeps, matching the pre-fix feel.
            virus_cure_resist: 2,
            virus_seed_rate: 0.004,
            worm_spawn_rate: 0.15,
            patch_wave_radius: 10,
            mutate_rate: 0.0008,
            mutate_min_age: 400,
            zero_day_period: 2000,
            zero_day_chance: 0.4,
            c2_spawn_bias: 0.6,
            fork_rate: 0.05,
            defender_pulse_period: 25,
            defender_radius: 5,
            // Default 1 keeps single-faction tests and library callers
            // simple. The CLI defaults to 2 so the released binary feels
            // like factions are "on".
            c2_count: 1,
            day_night_period: 600,
            night_spawn_mult: 1.6,
            night_loss_mult: 1.5,
            honeypot_backdoor_radius: 14,
            honeypot_backdoor_max: 3,
            storm_period: 1800,
            storm_chance: 0.35,
            storm_duration: 150,
            storm_spawn_mult: 2.2,
            storm_loss_mult: 2.2,
            ddos_period: 1200,
            ddos_chance: 0.4,
            ddos_stun_ticks: 60,
            wormhole_period: 800,
            wormhole_chance: 0.5,
            wormhole_life_ticks: 20,
            isp_outage_period: 2200,
            isp_outage_chance: 0.4,
            isp_outage_life_ticks: 180,
            isp_outage_min_side: 6,
            isp_outage_max_side: 14,
            partition_period: 2800,
            partition_chance: 0.45,
            partition_life_ticks: 220,
            sleeper_spawn_chance: 0.025,
            sleeper_wake_period: 100,
            sleeper_wake_chance: 0.35,
            tower_spawn_radius: 10,
            tower_pwn_resist: 2,
            ransom_chance: 0.15,
            carrier_chance: 0.10,
            drought_period: 2600,
            drought_chance: 0.35,
            drought_duration: 180,
            drought_hot_penalty: 6,
            defector_period: 900,
            defector_chance: 0.35,
            defector_intel_reward: 12,
            cross_faction_bridge_chance: 0.2,
            assimilation_period: 250,
            assimilation_threshold: 6,
            assimilation_dominance: 14,
            border_skirmish_period: 40,
            border_skirmish_radius: 3,
            border_skirmish_chance: 0.12,
            proxy_radius: 8,
            beacon_radius: 6,
            beacon_weight_mult: 1.5,
            epoch_period: 5000,
            mythic_big_one_threshold: 30,
            c2_count_max: 0,
            resurrection_threshold: 10,
            resurrection_chance: 0.55,
        }
    }
}

/// Pool of ominous names the sim draws from when assigning display
/// names to its STRAIN_COUNT virus strains at world start. Every run
/// picks 8 distinct names from this pool so the strains feel like
/// named threats instead of numbered enumerants.
pub const STRAIN_NAME_POOL: &[&str] = &[
    "Cerberus",
    "Hydra",
    "Phantom",
    "Wraith",
    "Basilisk",
    "Cobra",
    "Kraken",
    "Chimera",
    "Gorgon",
    "Banshee",
    "Lich",
    "Nyx",
    "Eris",
    "Hecate",
    "Tartarus",
    "Styx",
    "Omen",
    "Pandora",
    "Morrigan",
    "Azrael",
];

/// Named eras the sim cycles through as it ages. Each era name also
/// resolves to an `EraRules` block via `era_rules_for`, so crossing an
/// epoch boundary visibly rewrites the active tuning.
pub const ERA_NAMES: &[&str] = &[
    "Age of Silence",
    "First Signal",
    "Rise of the Mesh",
    "Era of Cascades",
    "Winter of Quarantine",
    "The Great Spreading",
    "Dusk Protocols",
    "Zero-Day Bloom",
    "Age of Wires",
    "Final Handshake",
    "Echo Chamber",
    "The Long Drift",
];

/// Per-era multiplier set consumed at the relevant tick-loop integration
/// points. All defaults are 1.0 (no effect); `era_rules_for` returns the
/// active set whenever the sim crosses into a new epoch. Keeping the
/// effects as a small struct of scalar multipliers means every call site
/// is a one-line change against the existing `self.cfg.*` read.
#[derive(Clone, Copy, Debug)]
pub struct EraRules {
    /// Scales `p_spawn` in `try_spawn`.
    pub spawn_mult: f32,
    /// Scales `p_loss` in `advance_dying`.
    pub loss_mult: f32,
    /// Scales `exfil_packet_period`. >1 = packets fire less often.
    pub exfil_period_mult: f32,
    /// Scales `virus_spread_rate` in the infection spread pass.
    pub virus_spread_mult: f32,
    /// Scales `mutate_rate` in `maybe_mutate`.
    pub mutate_mult: f32,
    /// Scales the post-cure `IMMUNITY_DURATION_TICKS` applied to nodes.
    pub immunity_mult: f32,
    /// Scales the cascade multiplier passed to `schedule_subtree_death`.
    pub cascade_mult: f32,
    /// Scales the effective assimilation cadence. >1 = more frequent.
    pub assimilation_speed_mult: f32,
    /// Scales `cross_faction_bridge_chance` in `maybe_reconnect`.
    pub bridge_mult: f32,
}

impl Default for EraRules {
    fn default() -> Self {
        Self {
            spawn_mult: 1.0,
            loss_mult: 1.0,
            exfil_period_mult: 1.0,
            virus_spread_mult: 1.0,
            mutate_mult: 1.0,
            immunity_mult: 1.0,
            cascade_mult: 1.0,
            assimilation_speed_mult: 1.0,
            bridge_mult: 1.0,
        }
    }
}

/// Map an epoch index to its mechanical `EraRules` plus a short summary
/// phrase used in the log on era transitions. Index wraps around
/// `ERA_NAMES.len()` so long runs cycle through the same rule sets.
pub fn era_rules_for(idx: usize) -> (EraRules, &'static str) {
    let base = EraRules::default();
    match idx % ERA_NAMES.len() {
        // "Age of Silence" — packets hush, loss eases.
        0 => (
            EraRules { exfil_period_mult: 2.0, loss_mult: 0.7, ..base },
            "packets hush, losses ease",
        ),
        // "First Signal" — growth surge.
        1 => (
            EraRules { spawn_mult: 1.3, ..base },
            "spawns surge",
        ),
        // "Rise of the Mesh" — bridges flourish alongside steady growth.
        2 => (
            EraRules { spawn_mult: 1.25, bridge_mult: 1.8, ..base },
            "bridges flourish",
        ),
        // "Era of Cascades" — cascades and losses amplified.
        3 => (
            EraRules { cascade_mult: 2.0, loss_mult: 1.3, ..base },
            "cascades 2× / loss 1.3×",
        ),
        // "Winter of Quarantine" — long immunity, weak spread.
        4 => (
            EraRules { immunity_mult: 5.0, virus_spread_mult: 0.4, ..base },
            "immunity 5× / spread 0.4×",
        ),
        // "The Great Spreading" — viral bloom.
        5 => (
            EraRules { virus_spread_mult: 2.2, ..base },
            "viral bloom (spread 2.2×)",
        ),
        // "Dusk Protocols" — losses climb, mutation stirs.
        6 => (
            EraRules { loss_mult: 1.25, mutate_mult: 1.5, ..base },
            "losses climb, mutation stirs",
        ),
        // "Zero-Day Bloom" — mutations rampant.
        7 => (
            EraRules { mutate_mult: 4.0, ..base },
            "mutation 4×",
        ),
        // "Age of Wires" — bridges multiply and packets accelerate.
        8 => (
            EraRules { bridge_mult: 2.5, exfil_period_mult: 0.7, ..base },
            "bridges 2.5× / packets 0.7×",
        ),
        // "Final Handshake" — assimilation accelerates.
        9 => (
            EraRules { assimilation_speed_mult: 3.0, ..base },
            "assimilation 3×",
        ),
        // "Echo Chamber" — echoes amplify both spread and cascades.
        10 => (
            EraRules { virus_spread_mult: 1.5, cascade_mult: 1.4, ..base },
            "echoes amplify",
        ),
        // "The Long Drift" — the mesh grows quiet.
        11 => (
            EraRules { spawn_mult: 0.6, loss_mult: 0.6, ..base },
            "the mesh grows quiet",
        ),
        _ => (base, ""),
    }
}

/// Adjective pool for procedurally-generated custom-event names.
/// The generator picks one adjective and one noun (plus a
/// template variation) to produce names like "the crimson
/// handshake" or "hollow requiem." Larger pools = more variety
/// per seed — with ~80 adjectives and ~80 nouns across ~7
/// templates, a three-word-slot generator has ~45k unique
/// surface names before repeats, enough that no two seeds
/// share a mythic vocabulary.
pub const EVENT_ADJECTIVES: &[&str] = &[
    // Colors and light
    "crimson", "obsidian", "amber", "violet", "emerald", "pale", "golden",
    "silver", "ashen", "scarlet", "cerulean", "ochre", "bronze", "indigo",
    // Darkness and decay
    "silent", "hollow", "broken", "forgotten", "cursed", "buried", "rotting",
    "withered", "derelict", "vacant", "crumbling", "severed", "splintered",
    // Light and reverence
    "radiant", "gilded", "sovereign", "solemn", "sacred", "anointed", "blessed",
    "exalted", "luminous", "hallowed",
    // Chill and quiet
    "frozen", "glacial", "whispered", "muted", "veiled", "shrouded", "cloaked",
    "mourning", "grieving", "dormant",
    // Heat and fury
    "smoldering", "burning", "feral", "ravenous", "wrathful", "seething",
    "blistering", "livid",
    // Iron and grit
    "iron", "leaden", "stoic", "tempered", "forged", "calloused", "rusted",
    "wired", "riveted",
    // Weather and time
    "twilight", "dusk", "dawn", "midnight", "autumnal", "winterbound",
    "eternal", "ephemeral", "timeless", "distant",
    // Edge and danger
    "jagged", "splintered", "bleeding", "razor", "shattered", "fractured",
    "untamed", "lawless", "errant",
];

/// Noun pool for procedurally-generated custom-event names.
/// Pairs with EVENT_ADJECTIVES via EVENT_NAME_TEMPLATES. Mix of
/// ritual/religious, procedural/legal, and warfare/journey
/// vocabulary so the generated names feel like they come from
/// different mythic traditions depending on the pick.
pub const EVENT_NOUNS: &[&str] = &[
    // Ritual and religion
    "pilgrimage", "vigil", "chorus", "covenant", "ritual", "conclave",
    "communion", "liturgy", "sermon", "matins", "vespers", "absolution",
    "confession", "benediction", "invocation", "hymn",
    // Procedural and legal
    "handshake", "verdict", "quorum", "writ", "edict", "doctrine", "accord",
    "treaty", "parley", "ledger", "tribunal", "ruling",
    // Arcane and secret
    "cipher", "sigil", "rune", "glyph", "oracle", "prophecy", "augury",
    "geas", "conjuring", "summoning",
    // Action and warfare
    "gambit", "harvest", "reckoning", "exodus", "procession", "siege",
    "onslaught", "ambush", "sortie", "reprisal", "purge", "incursion",
    // Judgment and ending
    "requiem", "elegy", "dirge", "lament", "eulogy", "epitaph",
    "threshold", "passage", "crossing",
    // Celestial and cosmic
    "eclipse", "apogee", "zenith", "equinox", "solstice", "aurora",
    "constellation",
    // Symbols and marks
    "pantheon", "canticle", "crown", "chalice", "mantle", "shroud",
    "relic", "icon", "brazier",
];

/// Name templates for custom events. Placeholders `{adj}` and
/// `{noun}` get substituted from the pools above at generation
/// time. Picking a template randomly per event means similar
/// adjective/noun pairs still read as varied — the user sees
/// "The crimson handshake" in one run and "Rite of the crimson
/// handshake" in another.
pub const EVENT_NAME_TEMPLATES: &[&str] = &[
    "the {adj} {noun}",
    "{noun} of the {adj} dawn",
    "{adj} {noun}",
    "rite of the {adj} {noun}",
    "the {noun}'s {adj} hour",
    "{adj} {noun} protocol",
    "hymn of the {adj} {noun}",
];

/// Per-layer physics modifier set. Stacks multiplicatively on
/// top of `EraRules` at every integration point: a faction on
/// mesh 1 during era 3 reads
/// `cfg * era.spawn_mult * layer.spawn_mult`. All defaults are
/// 1.0 (no effect) so layer 0 ("surface") is behaviorally
/// identical to the single-mesh sim from before the refactor.
#[derive(Clone, Copy, Debug)]
pub struct LayerRules {
    pub spawn_mult: f32,
    pub loss_mult: f32,
    pub virus_spread_mult: f32,
    pub mutate_mult: f32,
    pub reconnect_mult: f32,
}

impl Default for LayerRules {
    fn default() -> Self {
        Self {
            spawn_mult: 1.0,
            loss_mult: 1.0,
            virus_spread_mult: 1.0,
            mutate_mult: 1.0,
            reconnect_mult: 1.0,
        }
    }
}

/// Return the `LayerRules` for a given mesh index. Index 0 is
/// the surface layer (default rules), index 1 is the undernet
/// (sparse but infested), index 2 is orbital (fast and sparse).
/// Out-of-range indices fall back to defaults.
pub fn layer_rules_for(idx: usize) -> LayerRules {
    match idx {
        // Surface — baseline, no modifications.
        0 => LayerRules::default(),
        // Undernet — sparse spawns, durable nodes, virulent.
        1 => LayerRules {
            spawn_mult: 0.7,
            loss_mult: 0.8,
            virus_spread_mult: 1.4,
            mutate_mult: 1.6,
            reconnect_mult: 1.2,
        },
        // Orbital — very sparse, extremely low loss, calm.
        2 => LayerRules {
            spawn_mult: 0.5,
            loss_mult: 0.3,
            virus_spread_mult: 0.6,
            mutate_mult: 0.4,
            reconnect_mult: 2.0,
        },
        _ => LayerRules::default(),
    }
}

/// Names the sim awards to nodes that survive long enough and
/// spawn enough children to earn legendary status. Picked by
/// modular index off the node id so the same run produces the
/// same names deterministically for a given seed.
pub const LEGENDARY_NAME_POOL: &[&str] = &[
    "Orpheus",
    "Nyx-7",
    "Sable",
    "Vector",
    "Relic",
    "Ashkey",
    "Saturn",
    "Helix",
    "Monolith",
    "Quasar",
    "Obsidian",
    "Argus",
    "Crypt",
    "Vigil",
    "Warden",
    "Omega",
    "Pyre",
    "Revenant",
    "Shroud",
    "Zenith",
];
