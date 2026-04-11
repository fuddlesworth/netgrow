//! Pure data types for the simulation layer.
//!
//! This module holds the structs and enums that describe what's in
//! the world — nodes, links, infections, transient effects, stats —
//! separated from the `World` state machine and its tick logic,
//! which live in `mod.rs`. Keeping the type definitions here makes
//! `world/mod.rs` readable as a pure state-machine file.

use std::collections::VecDeque;

pub type NodeId = usize;

#[derive(Clone, Copy, Debug)]
pub enum State {
    Alive,
    Pwned { ticks_left: u8 },
    Dead,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Relay,
    Scanner,
    Exfil,
    Honeypot,
    /// Patrols its neighborhood and applies a local cure pulse to nearby
    /// infected nodes. Immune to infection itself; never mutates.
    Defender,
    /// Fortified core node with extra pwn-absorbing charges. Spawns only
    /// close to its faction's C2, creating visible fortified zones.
    Tower,
    /// Rally-point node that boosts nearby nodes' parent-selection weight,
    /// creating visible spawn clusters. Rendered with an always-on glow.
    Beacon,
    /// Scanner repeater — when any scanner within `proxy_radius` fires,
    /// this node also pulses, propagating the scanner highlight through
    /// a chain of proxies.
    Proxy,
    /// Looks like an exfil but never emits packets — a passive
    /// camouflage node that draws attacker attention away from real
    /// exfils. Rendered identically to an Exfil.
    Decoy,
    /// Traffic-caching junction. Absorbs exfil packets that reach it
    /// instead of forwarding them toward C2, easing congestion on the
    /// parent chain. Spawns rarely via role weights, or dynamically
    /// when a node's inbound link sustains hot traffic long enough
    /// that the congestion system upgrades it in place.
    Router,
    /// Internal hunter-killer. On its cooldown, scans adjacent
    /// same-faction neighbors for active infections and culls one
    /// by forcing it into the Pwned state — cutting off a strain's
    /// spread at the cost of a host. Counters Plague personas that
    /// otherwise have no friendly foil.
    Hunter,
}

impl Role {
    /// Lowercase display name used by log lines and the cursor
    /// inspector. Single place to add new role strings.
    pub fn display_name(&self) -> &'static str {
        match self {
            Role::Relay => "relay",
            Role::Scanner => "scanner",
            Role::Exfil => "exfil",
            Role::Honeypot => "honeypot",
            Role::Defender => "defender",
            Role::Tower => "tower",
            Role::Beacon => "beacon",
            Role::Proxy => "proxy",
            Role::Decoy => "decoy",
            Role::Router => "router",
            Role::Hunter => "hunter",
        }
    }

    /// Default render glyph with no state modifiers (no hardened
    /// override, no infection overlay). Used by infected_glyph and
    /// anywhere else that needs the plain shape for a role.
    pub fn base_glyph(&self) -> &'static str {
        match self {
            Role::Relay => "●",
            Role::Scanner => "◎",
            Role::Exfil => "▣",
            Role::Honeypot => "●",
            Role::Defender => "◇",
            Role::Tower => "⊞",
            Role::Beacon => "⊚",
            Role::Proxy => "⊛",
            Role::Decoy => "▣",
            Role::Router => "⊕",
            Role::Hunter => "⟁",
        }
    }

    /// Roles that never mutate — either because they hide (Honeypot),
    /// because their behavior is their identity (Defender, Tower,
    /// Beacon, Proxy), or because they're camouflage (Decoy).
    pub fn is_mutation_locked(&self) -> bool {
        matches!(
            self,
            Role::Honeypot
                | Role::Defender
                | Role::Tower
                | Role::Beacon
                | Role::Proxy
                | Role::Decoy
                | Role::Router
                | Role::Hunter
        )
    }

    /// Roles that can't be infected at all. Honeypots stay hidden;
    /// defenders are the antibody team; hunters are immune so they
    /// can keep culling without themselves being flipped.
    pub fn is_virus_immune(&self) -> bool {
        matches!(self, Role::Honeypot | Role::Defender | Role::Hunter)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InfectionStage {
    /// No visible symptoms yet, but still spreads.
    Incubating,
    /// Flickering glyph, normal role behavior suppressed.
    Active,
    /// About to crash the host — counts down `terminal_ticks` then forces a pwn.
    Terminal,
}

#[derive(Clone, Copy, Debug)]
pub struct Infection {
    pub strain: u8,
    pub stage: InfectionStage,
    pub age: u16,
    /// Decremented by patch waves; at 0 the infection is cured.
    #[allow(dead_code)]
    pub cure_resist: u8,
    pub terminal_ticks: u8,
    /// Ransomware variant: freezes the host instead of killing it at
    /// Terminal stage, and is immune to patch waves — only defender
    /// pulses can clear it.
    pub is_ransom: bool,
    /// Number of patch-wave hits this infection has already absorbed
    /// without being cured. Every `VETERAN_WAVE_THRESHOLD` survivals
    /// promote the infection — baseline `cure_resist` bumps up by
    /// one (up to `VETERAN_CURE_RESIST_CAP`), so strains that stick
    /// around become harder to clear. Resets on promotion so the
    /// counter ticks cleanly toward the next promotion.
    pub wave_survivals: u8,
    /// Promotion rank earned via veteran wave survivals. Purely a
    /// readout for the inspector / log lines — the mechanical
    /// effect is already baked into the inflated `cure_resist`.
    pub veteran_rank: u8,
}

impl Infection {
    pub fn seeded(strain: u8, cure_resist: u8) -> Self {
        Self {
            strain,
            stage: InfectionStage::Incubating,
            age: 0,
            cure_resist,
            terminal_ticks: 0,
            is_ransom: false,
            wave_survivals: 0,
            veteran_rank: 0,
        }
    }

    pub fn seeded_ransom(strain: u8, cure_resist: u8) -> Self {
        Self {
            strain,
            stage: InfectionStage::Incubating,
            age: 0,
            cure_resist,
            terminal_ticks: 0,
            is_ransom: true,
            wave_survivals: 0,
            veteran_rank: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Node {
    pub pos: (i16, i16),
    pub parent: Option<NodeId>,
    pub state: State,
    pub born: u64,
    pub pulse: u8,
    /// Nonzero means scheduled to die; render as red ✕ until it hits 0, then
    /// flip to Dead. Set via schedule_subtree_death with a delay proportional
    /// to distance from the pwned root, producing a visible red ripple through
    /// the subtree.
    pub dying_in: u8,
    pub role: Role,
    pub hardened: bool,
    pub heartbeats: u8,
    pub branch_id: u16,
    pub role_cooldown: u16,
    pub last_ping_dir: Option<(i8, i8)>,
    pub last_ping_tick: u64,
    pub honey_tripped: bool,
    pub honey_reveal: u8,
    /// Nonzero means a pwn attempt was just absorbed; renders as a bright
    /// shield glyph for a few ticks so the viewer sees the save happen.
    pub shield_flash: u8,
    pub infection: Option<Infection>,
    /// Nonzero means the node just mutated its role — flashes pink.
    pub mutated_flash: u8,
    /// Nonzero while a Scanner's ping pulse is active. The render pass
    /// uses this to brighten the scanner node itself and every link
    /// adjacent to it, creating a visible "surveying" pulse that rolls
    /// through the local topology instead of phantom dots in empty space.
    pub scan_pulse: u8,
    /// Extra pwn-absorbing charges beyond the normal heartbeat-driven
    /// shield. Towers spawn with a nonzero value; each pwn attempt
    /// decrements this counter before touching the regular hardened
    /// flag. Independent from `hardened` so towers can still gain and
    /// spend a heartbeat shield on top of their fortification.
    pub pwn_resist: u8,
    /// Which C2 this node belongs to (index into `World.c2_nodes`).
    /// Inherited from parent at spawn; first-hop C2 children take their
    /// C2's index. Used to keep cascade reachability and cross-link
    /// reconnects faction-isolated.
    pub faction: u8,
    /// Countdown of "ghost echo" ticks remaining after a node dies.
    /// While nonzero, the render pass draws the node's old role glyph
    /// at its last position in a dim color so the corpse is visible as
    /// a fading trace before the cell clears. Set when the node first
    /// transitions into `State::Dead`; decays in advance_role_cooldowns.
    pub death_echo: u8,
    /// Number of nodes spawned directly from this node as their
    /// parent. Feeds the legendary-node promotion rule along with
    /// the node's age — long-lived, heavily-reproductive nodes get
    /// a legendary name assigned and shown in the inspector.
    pub children_spawned: u16,
    /// Assigned once a node earns legendary status (age +
    /// children_spawned past the threshold). Indexes into the
    /// `LEGENDARY_NAME_POOL` so log lines and the inspector can
    /// render a stable callable name for recurring characters in a
    /// run. `u16::MAX` means "not legendary".
    pub legendary_name: u16,
    /// Some(f) marks this node as a sleeper agent secretly loyal
    /// to faction `f`. The node renders as its visible faction
    /// until `maybe_wake_sleepers` triggers, at which point it
    /// flips faction, infects its host neighborhood, and the field
    /// is cleared. None means the node is exactly what it appears
    /// to be.
    pub sleeper_true_faction: Option<u8>,
    /// Temporary post-cure immunity: when a node is cured of an
    /// infection, it records the strain id and a countdown. While
    /// the countdown is nonzero, spread and worm delivery of that
    /// specific strain cannot re-infect this node. Other strains
    /// can still land — immunity is strain-specific, not universal.
    pub immunity_strain: Option<u8>,
    pub immunity_ticks: u16,
}

impl Node {
    /// Active or Terminal infection suppresses this node's role behaviors
    /// (scanner pings, exfil packets). Incubating infections remain stealthy.
    pub fn role_suppressed(&self) -> bool {
        matches!(
            &self.infection,
            Some(i) if !matches!(i.stage, InfectionStage::Incubating)
        )
    }

    pub fn fresh(
        pos: (i16, i16),
        parent: Option<NodeId>,
        born: u64,
        role: Role,
        branch_id: u16,
    ) -> Self {
        Self {
            pos,
            parent,
            state: State::Alive,
            born,
            pulse: 0,
            dying_in: 0,
            role,
            hardened: false,
            heartbeats: 0,
            branch_id,
            role_cooldown: 0,
            last_ping_dir: None,
            last_ping_tick: 0,
            honey_tripped: false,
            honey_reveal: 0,
            shield_flash: 0,
            infection: None,
            mutated_flash: 0,
            scan_pulse: 0,
            pwn_resist: 0,
            faction: 0,
            death_echo: 0,
            children_spawned: 0,
            legendary_name: u16::MAX,
            sleeper_true_faction: None,
            immunity_strain: None,
            immunity_ticks: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    /// Tree edge created when a node is spawned from its parent.
    Parent,
    /// Lateral bridge between two live nodes in different branches. Used
    /// purely for cascade reachability — packets never relay through these.
    Cross,
}

#[derive(Clone, Debug)]
pub struct Link {
    pub a: NodeId,
    pub b: NodeId,
    pub path: Vec<(i16, i16)>,
    pub drawn: u16,
    pub kind: LinkKind,
    /// Accumulated traffic load. Each in-flight packet adds +2 per tick,
    /// each worm +1. Decays by 1 per tick. The renderer blends into
    /// hotter colors as load crosses WARM_LINK and HOT_LINK thresholds;
    /// packets refuse to hop onto a link whose load is above HOT_LINK.
    pub load: u8,
    /// Nonzero while this link is part of a recent exploit chain. Set
    /// when a pwn event walks up the parent chain from the victim
    /// toward C2; decays each tick in decay_link_load.
    pub breach_ttl: u8,
    /// Ticks of sustained HOT_LINK load. Climbs while the link is hot,
    /// unwinds while cool. Drives two congestion responses: crossing
    /// `ROUTER_UPGRADE_THRESHOLD` morphs the child endpoint into a
    /// Router; crossing `LINK_COLLAPSE_THRESHOLD` collapses the link,
    /// flushing traffic and quarantining it for a window.
    pub burn_ticks: u8,
    /// Nonzero during a post-collapse quarantine. Packets refuse to
    /// hop onto a quarantined link; decays -1 per tick in
    /// `decay_link_load`. Purely a gating counter — the visible effect
    /// is the flushed traffic and stunned endpoints.
    pub quarantined: u8,
    /// Lifetime count of exfil packets successfully delivered via
    /// this link (i.e. reached a Router cache or the final C2 hop
    /// without being dropped). Once this crosses
    /// `BACKBONE_PROMOTION_THRESHOLD` on a Parent link, the link
    /// earns backbone status: inflated HOT_LINK ceiling, distinct
    /// glyph, and cosmetic brightness.
    pub packets_delivered: u16,
    /// True once a Parent link has delivered enough traffic to
    /// qualify as a backbone. Flipped permanently — backbones stay
    /// backbones even if traffic dries up, so the label reads as
    /// "this chain mattered once".
    pub is_backbone: bool,
}

#[derive(Clone, Debug)]
pub struct Packet {
    pub link_id: usize,
    /// Index into link.path. Packets travel from the child end (high index)
    /// toward the parent end (index 0).
    pub pos: u16,
}

#[derive(Clone, Debug)]
pub struct Worm {
    pub link_id: usize,
    pub pos: u16,
    /// True if the worm started at `link.a` and is traveling toward `link.b`;
    /// false for the reverse. Cross-links are bidirectional so both are valid.
    pub outbound_from_a: bool,
    pub strain: u8,
    /// True if this worm is an antibody: instead of infecting its
    /// target, it cures any infection on arrival. Spawned by
    /// defender cures on a probabilistic roll. `strain` still
    /// carries a value for rendering purposes but isn't used as
    /// the delivered payload.
    pub is_antibody: bool,
}

/// AI personality assigned to each faction at world creation.
/// Biases its role_weights when a node spawns under it and
/// influences a few event rolls so factions read as distinct
/// players instead of interchangeable colored swarms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Persona {
    /// Information-gathering attacker. Higher Scanner / Exfil
    /// weights, lower Defender / Tower so its defenses are thin
    /// but its eyes are everywhere.
    Aggressor,
    /// Turtle defender. Higher Tower / Defender / Beacon weights,
    /// suppressed Exfil so the chain is fortified but quiet.
    Fortress,
    /// Viral spreader. Higher Exfil / Proxy weights, slightly
    /// suppressed Defender, and a virus_seed_rate multiplier so
    /// it leaks infections faster than the others.
    Plague,
    /// Balanced opportunist. Default weights, no per-role
    /// modifiers — the boring control case that keeps multi-
    /// faction runs from feeling like every faction is loud.
    Opportunist,
}

impl Persona {
    pub fn display_name(&self) -> &'static str {
        match self {
            Persona::Aggressor => "aggressor",
            Persona::Fortress => "fortress",
            Persona::Plague => "plague",
            Persona::Opportunist => "opportunist",
        }
    }
}

/// Two factions temporarily at peace. During the alliance, border
/// skirmishes between them are suppressed and cross-faction bridge
/// rolls between them don't fire. Purely a period of non-aggression.
#[derive(Clone, Copy, Debug)]
pub struct Alliance {
    pub a: u8,
    pub b: u8,
    pub expires_tick: u64,
}

/// Rare visual-only event: a dashed braille line flickering briefly
/// between two random alive mesh cells. Pure flavor, no effect on
/// routing or reachability.
#[derive(Clone, Debug)]
pub struct Wormhole {
    pub a: (i16, i16),
    pub b: (i16, i16),
    pub age: u16,
    pub life: u16,
}

/// Fixed-position fiber zone rolled at world creation. Nodes
/// spawned inside a hotspot start with bonus pwn_resist (a small
/// defensive head-start), and links whose cells overlap a
/// hotspot cool faster on traffic decay. Persistent — hotspots
/// live for the entire run so factions can fight over them.
#[derive(Clone, Debug)]
pub struct Hotspot {
    pub min: (i16, i16),
    pub max: (i16, i16),
}

impl Hotspot {
    pub fn contains(&self, pos: (i16, i16)) -> bool {
        pos.0 >= self.min.0
            && pos.0 <= self.max.0
            && pos.1 >= self.min.1
            && pos.1 <= self.max.1
    }
}

/// Rare environmental event — a horizontal or vertical slice
/// through the mesh that briefly blocks traffic from crossing it.
/// Packets and worms whose link spans both sides are dropped at
/// the partition. Thematic companion to IspOutage: instead of a
/// dead region, a line cut through the fabric.
#[derive(Clone, Debug)]
pub struct Partition {
    /// If true, the partition is a horizontal line at y = pos.
    /// Otherwise a vertical line at x = pos.
    pub horizontal: bool,
    /// Cell index (x or y depending on orientation) of the cut line.
    pub pos: i16,
    pub age: u16,
    pub life: u16,
}

impl Partition {
    /// True if the path from `a` to `b` crosses this partition line.
    /// The check is the classic 'points on opposite sides of the
    /// cut' test: whichever coordinate the partition slices, the
    /// two endpoints must straddle the line value.
    pub fn crosses(&self, a: (i16, i16), b: (i16, i16)) -> bool {
        if self.horizontal {
            (a.1 <= self.pos && b.1 > self.pos)
                || (a.1 > self.pos && b.1 <= self.pos)
        } else {
            (a.0 <= self.pos && b.0 > self.pos)
                || (a.0 > self.pos && b.0 <= self.pos)
        }
    }
}

/// Rare environmental event — a rectangular dead zone where new
/// spawns are blocked and any alive node inside has its role
/// cooldowns continuously spiked. Simulates an upstream provider
/// outage cutting off a region of the mesh. Lasts `life` ticks
/// then dissolves.
#[derive(Clone, Debug)]
pub struct IspOutage {
    /// Inclusive top-left and bottom-right cells of the dead zone.
    pub min: (i16, i16),
    pub max: (i16, i16),
    pub age: u16,
    pub life: u16,
}

impl IspOutage {
    pub fn contains(&self, pos: (i16, i16)) -> bool {
        pos.0 >= self.min.0
            && pos.0 <= self.max.0
            && pos.1 >= self.min.1
            && pos.1 <= self.max.1
    }
}

/// Rare sweeping event — a line of "hostile traffic" that moves across
/// the mesh from one edge to the opposite edge, spiking role cooldowns
/// on any node it passes over.
#[derive(Clone, Debug)]
pub struct DdosWave {
    /// Current position of the wave front (x or y, depending on orientation).
    pub pos: i16,
    /// If true, the wave is a horizontal line moving vertically; else
    /// a vertical line moving horizontally.
    pub horizontal: bool,
    /// +1 or -1, determines which way the front advances.
    pub direction: i16,
    pub age: u16,
}

/// Transient particle ejected from a cascade root. Sub-cell f32
/// position + velocity so multiple sparks in one terminal cell can
/// render as distinct braille dots.
#[derive(Clone, Debug)]
pub struct CascadeSpark {
    pub pos: (f32, f32),
    pub vel: (f32, f32),
    pub age: u8,
    pub life: u8,
}

/// Expanding shockwave ring drawn at a cascade root. Age increments
/// once per tick; radius tracks age directly. Cells on the ring get
/// rendered as bold braille chars.
#[derive(Clone, Debug)]
pub struct CascadeShockwave {
    pub origin: (i16, i16),
    pub age: u8,
    pub max_age: u8,
}

#[derive(Clone, Debug)]
pub struct PatchWave {
    pub origin: (i16, i16),
    pub radius: i16,
}

#[derive(Clone, Debug)]
pub struct RoleWeights {
    pub relay: f32,
    pub scanner: f32,
    pub exfil: f32,
    pub honeypot: f32,
    pub defender: f32,
    pub tower: f32,
    pub beacon: f32,
    pub proxy: f32,
    pub decoy: f32,
    pub router: f32,
    pub hunter: f32,
}

impl Default for RoleWeights {
    fn default() -> Self {
        Self {
            relay: 0.47,
            scanner: 0.13,
            exfil: 0.10,
            honeypot: 0.04,
            defender: 0.08,
            tower: 0.05,
            beacon: 0.04,
            proxy: 0.03,
            decoy: 0.02,
            router: 0.02,
            hunter: 0.02,
        }
    }
}

/// Running tally of notable events per faction. Used for the header
/// prestige readout and the end-of-run summary.
#[derive(Clone, Debug, Default)]
pub struct FactionStats {
    pub spawned: u32,
    pub lost: u32,
    pub honeys_tripped: u32,
    pub infections_cured: u32,
    /// Cumulative exfil packets that successfully delivered to this
    /// faction's C2 (or were absorbed by one of its Routers).
    /// Climbs monotonically over a run and feeds into the score so
    /// exfil deliveries have tangible long-term value.
    pub intel: u32,
    /// Recent alive-node count samples, bounded to FACTION_HISTORY_LEN.
    /// Sampled on a slow cadence so the header sparkline reads as a
    /// smooth trend rather than a jittering count.
    pub history: VecDeque<u32>,
    /// Highest alive-count this faction has ever reached. Used by the
    /// dynamic persona-shift rule: when current alive drops well
    /// below the peak, the faction flips to a defensive persona.
    pub peak_alive: u32,
}

impl FactionStats {
    /// Composite "prestige" score. Positive for growth, negative for
    /// churn. Honeypot traps and cures are rewarded so defense-
    /// leaning factions can still score well without hoarding nodes.
    pub fn score(&self) -> i32 {
        self.spawned as i32 - 3 * (self.lost as i32)
            + 5 * (self.honeys_tripped as i32)
            + 2 * (self.infections_cured as i32)
            + 3 * (self.intel as i32)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WorldStats {
    pub alive: usize,
    pub pwned: usize,
    pub dead: usize,
    pub dying: usize,
    pub branches: usize,
    pub factions: usize,
    pub links: usize,
    pub cross_links: usize,
    pub packets: usize,
    pub infected: usize,
}
