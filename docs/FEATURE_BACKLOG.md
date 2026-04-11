# netgrow feature backlog

Compiled design report of every feature that has shipped in netgrow, every
unshipped idea we've brainstormed, and recommended next directions. Updated
as rounds complete.

## Design philosophy

netgrow is a **hands-off ambient simulation** — the viewer watches the
faction dynamics emerge on their own, and the mesh runs indefinitely until
you quit. New features should preserve this: the sim decides what happens,
the viewer reads it. No win conditions, no mandatory interaction, no
modal game states.

**God-mode tools** are allowed as inspection + light influence: the
cursor-action hotkeys, faction favoritism, and event injection give the
viewer ways to *nudge* what they're watching without taking control of any
faction. Think of it like a zoologist poking a tank — you can reach in,
but the animals run the show. Any future feature that would require the
viewer to "play" a faction directly is out of scope.

**How to read this**: status markers mean
- ✅ shipped
- 🔨 in flight / partially done
- (no marker) proposed, unshipped

---

## Table of contents

1. [Shipped features](#shipped-features)
2. [Round 2: small and medium ideas](#round-2-small-and-medium-ideas)
3. [Round 3: major system expansions](#round-3-major-system-expansions)
4. [Recommended combos](#recommended-combos)
5. [Next-up recommendations](#next-up-recommendations)

---

## Shipped features

These are all the features we've implemented. If you're looking for "what
already exists in the sim", this is the authoritative list.

### Core layer (always present)

- ✅ **Multi-faction competition** — 1–4 starting C2s, each the root of a
  growing subtree. Assimilation collapses weak factions into strong ones.
- ✅ **Branch tree growth** — nodes spawn from a parent chain with
  configurable fork rates; each branch inherits or forks from its parent.
- ✅ **11 role system** — Relay, Scanner, Exfil, Honeypot, Defender, Tower,
  Beacon, Proxy, Decoy, Router, Hunter, each with its own firing behavior.
- ✅ **Role synergies** — adjacent role combos unlock bonuses (Tower+Defender
  regenerates shields, Scanner+Beacon doubles scan pulse, Exfil+Router lifts
  backpressure ceiling).
- ✅ **Routed link geometry** — box-drawing paths between parent/child nodes
  with real routing. Backbone links get thicker glyphs when load justifies it.
  Paths cross over each other freely.

### Faction & persona layer

- ✅ **Faction personas** — each faction rolls Aggressor/Fortress/Plague/
  Opportunist at spawn; role-weight biases, event biases, and log flavor
  derived from persona.
- ✅ **Dynamic persona shifts** — factions flip persona based on alive count
  vs peak (e.g. losing a cascade triggers Turtle mode).
- ✅ **Faction-shuffled palette** — each run permutes the 12-color faction
  palette so which hue represents which faction varies per seed.
- ✅ **Rivalries** — per-pair war pressure that accumulates from kills,
  skirmishes, cross-faction worm crossings.
- ✅ **War declarations** — rivalry crossing threshold fires `✦ WAR ✦`
  mythic + 3x border skirmish amp for a window.
- ✅ **Alliances** — peace treaties that suppress skirmishes and
  cross-faction bridging between signatories.
- ✅ **Assimilation** — weak factions below the dominance threshold get
  absorbed by strong ones, surviving nodes reparented to nearest strong
  neighbor.
- ✅ **Dominance tracking** — faction >=60% alive or last C2 standing gets
  a live dominance badge; never auto-exits.
- ✅ **C2 HP / worm siege** — each C2 ships with 200 pwn_resist; enemy
  worms drain 8 per hit; at zero the C2 falls and its whole subtree
  cascades.
- ✅ **Resurrection** — large cascades (10+ doomed) have a 55% chance to
  pick one node and rise it as a new C2 with its own persona + palette
  slot + HP.

### Virus / infection layer

- ✅ **Strains** — 8 named strains, deterministic per seed.
- ✅ **Stages** — incubating → active → terminal.
- ✅ **Ransomware variants** — freeze hosts indefinitely, immune to patch
  waves, only defender pulses clear.
- ✅ **Veteran promotions** — surviving patch waves bumps cure_resist up
  to a cap, making strains harder to clear as they age.
- ✅ **Strain merging** — worm landing on infected host with a different
  strain produces a deterministic hybrid combining both cure_resist
  values.
- ✅ **Strain ecology** — weaker strains outcompeted by dominant ones at
  1%/tick when the dominant has 2x+ strength.
- ✅ **Post-cure immunity** — cured nodes gain 180-tick strain-specific
  immunity window, creating visible "cured pockets".
- ✅ **Antibody worms** — Defender cures probabilistically launch reversed-
  color ◈ worms that travel existing links and cure targets on arrival.
- ✅ **Hunter cull** — Hunter role periodically exploits adjacent same-
  faction infected neighbors to cut off strain spread.
- ✅ **Zero-day events** — periodic global events: outbreak, emergency
  patch, or immune breakthrough.
- ✅ **Sleeper agents** — rare spawns that look like any ordinary role but
  secretly belong to another faction. Wake on a probabilistic trigger and
  flip allegiance.

### Traffic / congestion layer

- ✅ **Packets** — fired by Exfils on cooldown, hop parent-chain toward C2.
- ✅ **Intel accumulation** — each delivered packet credits the faction's
  `intel` counter. Folds into score.
- ✅ **Link load + backpressure** — every in-flight packet heats its
  carrier; hot links refuse new packets; warm links trigger exfil
  backpressure.
- ✅ **Cross-link rerouting** — congested packets scan cross-links for
  cooler routes and jump onto them.
- ✅ **Router absorption** — Routers cache ~65% of arriving packets,
  letting ~35% pass through.
- ✅ **Backbone promotion** — Parent links that deliver enough packets
  earn backbone status with inflated HOT_LINK ceiling and thicker glyphs.
- ✅ **Link overload collapse** — links sustaining HOT traffic past a
  threshold flush all traffic, stun endpoints, and quarantine for a
  window. Logs as `⚡ LINK OVERLOAD`.
- ✅ **Scanner sightings** — Scanners probabilistically log enemy-faction
  assets spotted in range.

### Environmental events

- ✅ **Day/night cycle** — spawn/loss oscillations.
- ✅ **Storms** — a rolling directional front sweeps across the mesh from
  the top edge. Spawn/loss rates spike for the duration.
- ✅ **DDoS waves** — a line of hostile traffic sweeps an edge-to-edge
  pass stunning every node it passes over.
- ✅ **Wormholes** — dashed rift flickers briefly between two random
  alive cells.
- ✅ **ISP outages** — rectangular dead zones blocking spawns and stunning
  alive nodes inside.
- ✅ **Network partitions** — horizontal/vertical line cuts through the
  mesh that drop packets and worms trying to cross them.
- ✅ **Fiber hotspots** — persistent fixed terrain zones rolled at world
  creation. Nodes spawned inside gain bonus pwn_resist.

### Narrative layer

- ✅ **Legendary nodes** — surviving 1200+ ticks with 8+ children earns a
  random name from a pool and a bold/underlined render highlight.
- ✅ **Legendary biographies** — deterministic one-line bios generated at
  render time from templates, shown in inspector + summary.
- ✅ **Eras** — epoch names cycle through a pool as the run ages, shown
  in mesh title and log (but currently flavor-only — no mechanical
  effect).
- ✅ **Mythic events** — `✦ WAR ✦`, `✦ DOMINANCE ✦`, `✦ FALL ✦`,
  `✦ MYTHIC ✦ THE BIG ONE`, `✦ MYTHIC ✦ PANDEMIC`, `✦ MYTHIC ✦ REBIRTH`,
  all colorized distinctly in the log.

### God-mode tools

- ✅ **Cursor mode** — Tab toggles inspector cursor, arrow keys move it.
- ✅ **Cursor-action hotkeys** — `i`/`p`/`s`/`c`/`w` at cursor (infect,
  patch wave, scanner pulse, plant C2, wormhole).
- ✅ **Faction favoritism** — 1–9 key boosts a faction's spawn weight for
  300 ticks.
- ✅ **View mode toggle** — `v` cycles between Runtime and Intel views.
- ✅ **Pause / speed control** — space pauses, +/- adjusts tick period.

### Rendering / UX

- ✅ **Runtime view** — stats, activity graph, faction leaderboard, roles
  legend, logs, inspector.
- ✅ **Intel view** — minimap with C2 markers + hotspot overlays + cursor
  position, rivalries heatmap, active events panel, inspector, logs.
- ✅ **Themes** — 7 built-in themes (cyberpunk, aretha-dark, nord, gruvbox,
  dracula, catppuccin-mocha, solarized-dark), custom TOML loader.
- ✅ **Faction-colored mesh** — nodes and links draw in their faction's
  hue, no branch color noise.
- ✅ **Hotspot + outage bg tints** — fixed strategic terrain shows through
  the mesh regardless of ownership.
- ✅ **Ricer end-of-run summary** — ASCII banner, meta panel, totals panel,
  sorted leaderboard with medals + score bars, legendary roll call.
- ✅ **Log colorization** — every mechanical event gets its own styled
  color; mythic events get reversed / bg backgrounds.

---

## Round 2: small and medium ideas

Brainstormed unshipped items from earlier rounds, rough scope markers:

### Economy / scarcity

- ✅ **Bandwidth Drought** — periodic event (`maybe_drought`) that
  tightens the effective HOT_LINK ceiling by `drought_hot_penalty`
  so routing refuses moderately-loaded links and exfils throttle
  sooner. `effective_hot_link(link)` helper centralizes the
  ceiling calculation across packet routing. Render shows a
  `⚠ drought (Nt)` row in the events panel.
- ✅ **Ghost Packets** — `Packet.ghost` flag. Opportunist exfils
  emit decoys 25% of the time, Plague 20%. Decoys add link load
  and pollute Router caches but skip the intel reward on delivery.
  Render draws ghosts in the ghost palette color without BOLD.
- ✅ **Defector Events** — periodic `maybe_defector` picks a non-C2
  alive node, flips it to a random rival faction, reparents to the
  nearest same-faction anchor, and credits the receiving faction
  with `defector_intel_reward` (default 12) intel as "topology
  memory carried across the lines." Log mythic formatted as
  `✦ MYTHIC ✦ F{a} → F{b} defector @ 10.0.x.y (+N intel)`.
- **Mercenary Nodes** *(small)* — unaffiliated auction-bidding nodes that
  sell to the highest bidder each cycle. Compounds faction dominance.
- ✅ **Strain Patents** — `World.strain_patents: Vec<Option<u8>>`
  (indexed by strain id). When a worm-collision hybrid forms, the
  merge target's faction claims the patent. Every sample period,
  `collect_strain_patents` grants the owner +1 intel per rival-
  faction host carrying the strain. Logs `✦ patent ✦ F{N} files
  claim on {strain}` on ownership change. Patents clear when an
  owning faction dies.
- **Bandwidth Drought** *(small)* — environmental event drops total link
  capacity region-wide for N ticks. Forces traffic prioritization.
- **Black Market Links** *(small)* — temporary unlicensed high-bandwidth
  backbones spawned by Opportunist factions, collapse under ISP pressure.

### Diplomacy / social

- **Cold War Pacts** *(small)* — non-aggression treaty with a hidden
  betrayal timer. Adds paranoia.
- **Defector Events** *(small)* — a node flips faction carrying partial
  knowledge of its old subtree topology. Counter-intel asymmetry.
- **Syndicate Votes** *(medium)* — when 3+ factions are balanced, a rare
  cartel vote pile-on fires against one rival. Anti-dominance valve.

### Virus subtlety

- ✅ **Carrier Nodes** — new `Infection.is_carrier` variant.
  Skips the Active → Terminal transition entirely so the host
  stays infected indefinitely (spreads normally but never
  crashes). Seeded via `carrier_chance` (default 0.10), mutually
  exclusive with ransom. Inspector virus row shows `CARRIER`
  badge; seed log colorized with header_brand bg.

### Lifecycle / cascades

- ✅ **Scorched-Earth Protocol** — a faction that drops below 25% of
  its peak (with peak >= 20) rolls a 40% chance per sample period to
  self-destruct its own subtree from the C2 down via
  `schedule_subtree_death(c2_id, 2.0)`. Logged as
  `✦ SCORCHED EARTH ✦ F{N} initiates total collapse`. One-shot per
  faction lifetime via `FactionStats.scorched_earth_fired`.
- ✅ **Faction Memory Decay** — when a C2 dies in `advance_dying`,
  all rivalry entries and active wars involving that faction id are
  purged from the HashMaps. Logs `F{N} memory fades — {N} rivalries
  forgotten` in ghost color.

### Terrain / topology

- ✅ **Sleeper Lattice** — new `Link.latent` flag. Some
  reconnect links (~25% same-faction rolls) are created dormant:
  invisible in render, skipped in cascade reachability, packet
  reroute, worm traversal. `activate_sleeper_lattice` runs every
  sample period and wakes sleepers whose endpoints' factions are
  at war OR whose parent chain is dead/dying. Logs
  `✦ lattice ✦ sleeper edge wakes` in reversed cross_link color.

### Traffic / deception

- **Ghost Packets** *(trivial)* — decoy traffic clogs rival router
  caches with fake flows. Cheap harassment tool.

### Narrative / history

- ✅ **Lore Tablets** — legendary nodes render as permanent `†`
  tombstones on death (held in `occupied` forever, echo pinned at
  max). Their full connection web stays visible in dim accent
  color as a permanent memorial. Fall logs fire
  `✦ legend ✦ {name} falls @ ip (F{N})` as mythic events.

### Viewer nudges

- **Turf Graffiti** *(trivial)* — mark a cell as high-value target for
  one cycle. Light-touch god-mode steering — the sim still decides, the
  mark just biases role weights toward that cell briefly.

### Visualization

- **Spectral View Mode** *(small)* — new view toggle overlaying dead
  nodes, last-known routes, and previous-cycle territory. Reading the
  shadow map reveals momentum.

---

## Round 3: major system expansions

Bigger swings that would change what netgrow fundamentally is or does. All
are implementable but each is a multi-commit undertaking at minimum.

### ✅ 1. Full diplomatic state machine *(L)*

Shipped: `World.wars`, `World.alliances`, `World.rivalry` all folded
into a single `World.relations: HashMap<(u8,u8), Relation>` keyed by
canonical pair. Each relation carries `state: DiplomaticState`,
`pressure`, `trust`, `entered_tick`, and `expires_tick`. Seven states
compose a ladder:

- **Neutral** — default, pressure decays quietly
- **Trade** — periodic opportunistic roll between low-pressure pairs,
  Opportunist-biased, accumulates trust toward NAP
- **NonAggression** — upgrades from Trade at `NAP_TRUST_THRESHOLD`,
  continues trust-building toward Alliance
- **Alliance** — upgrades from NAP at `ALLIANCE_TRUST_THRESHOLD`,
  strongest peaceful bond
- **ColdWar** — promoted from Neutral at `COLD_WAR_THRESHOLD`,
  skirmishes dampened but sleeper lattice wakes readily
- **OpenWar** — promoted from ColdWar at `WAR_DECLARATION_THRESHOLD`
  (lowered by 20 for Aggressor personas), 3× skirmish amp +
  cross-faction bridge freedom + worm crossing allowed, on expiry
  falls back to ColdWar
- **Vassalage { overlord }** — rolled during OpenWar when one side
  is >2× the other and the weaker is below 30% of its peak; vassal
  pays +1 intel tribute each sample period to the overlord; breaks
  back to ColdWar if the vassal recovers past 70% of the overlord

Persona biases: Aggressor lowers war threshold, Fortress/Opportunist
boost trust gain rate, Opportunist boosts Trade roll odds. Betrayal
mechanic: hostile pressure in a peaceful state snaps straight to
OpenWar with a trust plummet. Back-compat helpers `at_war`, `allied`,
`rivalry_pressure`, `bump_rivalry` all preserved — callers don't
know the backing store changed. Render panel upgraded from
"rivalries" to "diplomacy": sorted by state priority (war first,
then vassalage/cold/alliance/NAP/trade), each row shows the state
label + pressure bar.

### ✅ 2. Era system with mechanical rules *(M-L)*

Shipped: `EraRules` struct + `era_rules_for(idx)` mapping in
`config.rs`. Each of the 12 named eras now rebinds a set of scalar
multipliers (`spawn_mult`, `loss_mult`, `exfil_period_mult`,
`virus_spread_mult`, `mutate_mult`, `immunity_mult`, `cascade_mult`,
`assimilation_speed_mult`, `bridge_mult`) consumed at the relevant
tick-loop sites. Era transitions fire a log line with a short summary
suffix (e.g. `── era 3: Era of Cascades ✦ cascades 2× / loss 1.3×`).
The opening era ("Age of Silence") hushes packets and eases losses;
"Winter of Quarantine" stretches post-cure immunity 5×; "Zero-Day
Bloom" quadruples mutation; "Final Handshake" accelerates assimilation;
"The Long Drift" quiets both spawn and loss.

### ✅ 3. Hierarchical tech tree per faction *(L)*

Shipped: `FactionStats` gained `research`, `tech_tier`,
`last_intel_sample`, `last_cures_sample`. `advance_research` runs on
the faction sample cadence and computes per-faction income from
`base + 3·intel_delta + 2·cures_delta`, then applies a diplomatic
multiplier (Trade +25%, NAP +10%, Alliance +50%) and skims 30% of
each vassal's income to their overlord before adding to the research
counter. Three tier thresholds (`TECH_TIER_1_COST=100`, `_2=300`,
`_3=800`) promote the faction's `tech_tier` and emit a
`✦ tech ✦ F{N} {persona} reaches tier {T} — {effect}` log line.

**Tier 1** — `tech_role_intensity` amplifies the persona's role-weight
deviation from 1.0 (intensity 1.35 at T1, 1.6 at T2/T3), read by
`roll_role` when a new node picks its role. Stronger factions feel
more like their persona.

**Tier 2** — persona-specific passive:
- Aggressor → `tech_scanner_period_mult=0.65` (scanners fire 35%
  faster)
- Fortress → `tech_defender_radius_bonus=+2` (wider cure pulses)
- Plague → `tech_worm_spawn_mult=2.0` (doubled worm emission from
  its infected hosts)
- Opportunist → `tech_bridge_mult=2.0` (doubled cross-faction bridge
  roll)

**Tier 3** — active ability rolled at `TECH_T3_ACTIVE_CHANCE=0.22`
per sample period:
- Aggressor → `tech_active_aggressor`: free extended scanner pulse on
  one of its scanners (`✦ tech ✦ F{N} orbital sweep @ 10.0.a.b`)
- Fortress → `tech_active_fortress`: free patch wave from one of its
  defenders (`✦ tech ✦ F{N} pulse cannon @ 10.0.a.b`)
- Plague → `tech_active_plague`: plants 2 fresh infections anywhere
  on the mesh (`✦ tech ✦ F{N} endemic bloom — 2 hosts seeded with
  {strain}`)
- Opportunist → `tech_active_opportunist`: spawns a free wormhole
  (`✦ tech ✦ F{N} brokered wormhole @ 10.0.a.b`)

Faction leaderboard row shows a compact tier glyph
(`····`/`t1▪▪`/`t2▪▪▪`/`t3★`) between persona and alive-count so the
reader can see at a glance which factions are ahead. Log colorizer
tints `✦ tech` lines in accent color.

**Sabotage** is deferred — the hook points exist (`research` is just
a counter) but the first cut ships without it. Future work: in
OpenWar state, skirmish kills shave a small amount off the victim's
research.

### 4. Multi-mesh / layered networks *(XL)*

Instead of a single flat grid, several smaller meshes connected by
long-distance "backbone" links. Factions operate primarily on one mesh
but can send raids across. Or a second layer stacked on top (dark web /
orbital / etc) with different physics. Tab key switches primary view
between meshes.

**Transforms**: World feels bigger; cross-mesh escalation becomes its own
arc.

### ✅ 5. Civil wars / faction fission *(L)*

Shipped: `Persona::aligned_roles` / `divergent_roles` define each
persona's ideological core (Aggressor: Scanner/Exfil/Hunter;
Fortress: Tower/Defender/Beacon; Plague: Exfil/Proxy/Router;
Opportunist: none). `advance_fission` runs on the faction sample
cadence, groups alive non-C2 nodes by `(faction, branch_id)`,
scores each group's divergent-role fraction, and rolls
`FISSION_ROLL_CHANCE=0.18` on the most divergent qualifier if the
branch is ≥15 nodes and ≥50% divergent. On a hit, `execute_fission`
promotes the branch's most-connected node to C2, reparents every
other member to it, cuts the hub's old parent chain (dropping the
link), allocates a fresh faction id with a persona derived from
the branch's actual role mix (majority-signature-role wins), and
seeds a pre-loaded OpenWar relation with the parent at
`FISSION_INITIAL_PRESSURE=150` pressure and `trust = -TRUST_CAP`.
Emits `✦ MYTHIC ✦ F{new} splinters from F{parent} ({N} persona)
@ 10.0.a.b`. Opportunist factions never fission (no divergent
roles). Interacts with tech tree (splinter starts at T0 with fresh
FactionStats) and the diplomacy FSM (auto-war already in place).

### 6. Replay system *(XL)*

Record every World state mutation to a ring buffer. Press `[` / `]` to
scrub backward/forward through time. Pin a tick as a reference point and
replay forward from it. Purely observational — no interaction with the
past, just inspection.

**Transforms**: Lets the viewer deeply inspect what just happened. Turns
the sim into a tool you can *study*.

### 7. Procedural event generator *(L)*

Compose events from parts at world creation: pick a random trigger
(rivalry > threshold / era matches / day_night state / node count > X),
a random condition (faction id / role / infection state), and a random
effect (cascade / spawn boost / cure / event cascade). Produces
brand-new event types each run that have literally never existed before.
Names generated from a template pool.

**Transforms**: Every run has its own mythic events the viewer encounters
for the first time. Never feels stale.

---

## Recommended combos

### From Round 2

- **Virus escalation** — Carrier Nodes + Strain Patents + Sleeper Lattice
- **Narrative/history** — Lore Tablets + Scorched-Earth + Faction Memory
  Decay *(coherent "map remembers" theme, all small/trivial)*
- **Viewer nudges** — Turf Graffiti + Mercenary Nodes + Cold War Pacts
  *(three tiny levers that bias the sim without taking control)*
- **Economy** — Bandwidth Drought + Black Market Links + Strain Patents

### From Round 3

- **Deepen what's there** — Era system + Hierarchical tech tree + Civil
  wars *(these all add depth without changing what netgrow fundamentally
  is; each makes factions feel more alive and autonomous)*
- **Study the sim** — Replay system + Procedural event generator
  *(turn the sim into a tool you can rewind and inspect while every
  run still generates first-time events)*
- **Cross-round** — Era system + Procedural event generator + Scorched-
  Earth Protocol *(turns long runs into acts of emergent narrative where
  each phase has its own rules, mythics, and dramatic exits)*

---

## Next-up recommendations

### If we're continuing with Round 2 small items

**Narrative/history combo**: Lore Tablets + Scorched-Earth + Faction
Memory Decay. Three coherent commits, all small/trivial, form a
"map remembers" theme.

### If we're jumping into Round 3 major work

**Era system with mechanical rules** (#2) as the first major expansion.
Rationale:
- Eras already exist as flavor text and a named pool
- Hooks into the existing tick loop at one clean point
- Every existing system can have its constants swapped per era without
  architectural changes
- Long runs get visible narrative acts — viewers *feel* the phase shifts
- No new UI concepts needed (era badge already renders in mesh border
  title)
- Achievable in a focused 3–4 commit sequence
- Doesn't conflict with any other Round 3 idea — acts as a foundation
  for e.g. procedural event generator later

### The single biggest creative stretch

**Procedural event generator** (#7). Produces the most "first-time-I've-
seen-this" moments across runs. Slightly higher complexity than the era
system because it needs a mini event DSL, but still in L range.

### The single biggest structural shift

**Multi-mesh / layered networks** (#4). Expands the world from one grid
to several connected meshes with cross-mesh raids — the biggest change
to what netgrow *is* that still respects the hands-off sim framing.

---

## Scope summary

| Round | Count | Total scope |
|---|---|---|
| Shipped | 60+ features | — |
| Round 2 unshipped | 16 ideas | Mostly trivial/small |
| Round 3 unshipped | 7 ideas | L to XL each |

The sim is feature-rich enough that every Round 2 item is a polish
pass on existing systems, while Round 3 items are genuine expansions
that would change how the sim behaves at a structural level. Either
set is worth shipping independently; together they would mature the
project into something far beyond its current scope.
