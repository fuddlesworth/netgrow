# netgrow

A cyberpunk botnet / mesh simulation that grows, fights, infects, and collapses in your
terminal. Watch competing factions of command-and-control networks spread across a
terminal grid, form alliances, declare wars, infect each other with evolving viral
strains, and rise and fall as one takes dominance.

Built in Rust with [ratatui](https://github.com/ratatui/ratatui) and
[crossterm](https://github.com/crossterm-rs/crossterm). No installation required beyond
`cargo run`.

## Screenshots / feel

A typical run looks something like this (all rendered in the terminal):

```
 ┌───────────────── era: Echo Chamber ─────────────────┐    ┌ stats ─────────────────────┐
 │    ●───●───◉                           ⊚─●─◉       │    │ alive    92   pwned    312 │
 │    │   │   │             ●──◎─●                    │    │ dying     0   dead     147 │
 │    │   ◇   ●──●     ●───●     │        ▣───●       │    │ branches 18   bridges    6 │
 │    ●───●   │   │    │   ⊞     ●───◉                │    │ infected  4   packets   21 │
 │        │   ●   ⊛    ●─◆───●       │                │    │ routers   2   legends    1 │
 │        ⊕           │     │        ●───⟁            │    └────────────────────────────┘
 │                    ●     ●                         │    ┌ activity ──────────────────┐
 │                                                    │    │  ⣀⣤⣶⣶⣶⣦⣤⣀⣀⣠⣴⣶⣶⣿⣶⣶⣶⣶⣶⣶⣤⣤⣠⣤⣶⣶⣿ │
 │                                                    │    │  ⣴⣶⣿⣿⣿⣿⣿⣿⣶⣴⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ │
 └────────────────────────────────────────────────────┘    └────────────────────────────┘
```

Nodes spawn from a handful of C2 roots, grow branching network trees, pick
specialized roles, and play out an emergent strategy game between 2–8 factions — each
with its own personality and color palette.

## Quick start

```bash
# from the repo root
cargo run --release
```

Quit with `q`. On first launch the sim rolls a random seed and grows a default-config
mesh. Every run is deterministic for a given seed.

```bash
# reproduce a specific run
cargo run --release -- --seed 42

# different theme
cargo run --release -- --theme aretha-dark

# slower ticks
cargo run --release -- --tick-ms 120

# load a config file
cargo run --release -- --config ~/.config/netgrow/config.toml
```

## Keybinds

### Default mode (no cursor)

| Key | Action |
|-----|--------|
| `q` | Quit and show the end-of-run summary |
| `space` | Pause / resume the simulation |
| `+` / `-` | Speed up / slow down (`tick_ms`) |
| `Tab` | Toggle inspector cursor on / off |
| `v` | Cycle view mode (`runtime` / `intel`) |
| `1`–`9` | Favor that faction — 2.5× spawn-roll boost for 300 ticks |

### Cursor mode (press `Tab` first)

| Key | Action |
|-----|--------|
| `←↑↓→` | Move the inspector cursor |
| `Tab` | Exit cursor mode |
| `i` | Inject a fresh infection at the nearest alive node |
| `p` | Drop a patch wave centered on the cursor |
| `s` | Fire a scanner pulse from the nearest alive node |
| `c` | Plant a new C2 (new faction) at the cursor cell |
| `w` | Spawn a wormhole connecting cursor to a random alive cell |

## View modes

Press `v` to flip between the two right-column layouts.

- **Runtime** — `stats / activity / factions / roles / (inspector) / logs`. The
  default live-game view.
- **Intel** — `minimap / rivalries / events / (inspector) / logs`. Info-dense state
  readout: a birds-eye minimap of faction territory, a sorted list of hot rivalry
  pairs with pressure bars and war markers, and a live list of active environmental
  events (storms, DDoS waves, ISP outages, partitions, wars).

## The mesh

### Node lifecycle

A node can be:

- **Alive** — growing, firing its role, picking up heartbeats, eventually hardening.
- **Pwned** (`◉`) — freshly exploited, transient red-block flash for ~18 ticks before
  the cascade path picks it up.
- **Dying** (`✕`) — scheduled for subtree death with a per-hop delay, producing a
  visible red ripple through the subtree.
- **Dead** (`·`) — gone, but legendary-node ghost echoes render the old role glyph
  dimmed for a window before the cell clears.

### Roles

Every non-C2 node rolls a role at spawn from weighted odds that are further biased
by its faction's persona:

| Glyph | Role | What it does |
|-------|------|--------------|
| `●` / `◉` | Relay / hardened | Baseline growth substrate; hardens after enough heartbeats. |
| `◎` | Scanner | Fires periodic pings that pulse the local topology and probabilistically spots enemy assets in range. |
| `▣` | Exfil | Fires data packets toward C2 on a cooldown. Each delivered packet credits the faction's intel counter. |
| `●` | Honeypot | Looks like a relay; when pwned it reveals nearby backdoors and triggers an oversized cascade. |
| `◇` | Defender | Immune to viruses; periodic pulses cure nearby infected nodes and can launch antibody worms. |
| `⊞` | Tower | Hardened core that spawns only near C2; extra pwn_resist charges regenerate when adjacent to a Defender. |
| `⊚` | Beacon | Rally point; boosts nearby spawn weight and doubles adjacent Scanner pulse duration. |
| `⊛` | Proxy | Scanner repeater — echoes scanner pings through chains of connected proxies. |
| `▣` | Decoy | Looks exactly like an Exfil but never emits packets; pure camouflage. |
| `⊕` | Router | Caches packets that reach it instead of forwarding to C2, bleeding pressure off hot parent chains. |
| `⟁` | Hunter | Periodically culls adjacent same-faction infected neighbors to cut off strain spread. |

Adjacent role combos unlock synergies: Tower + Defender regenerates shields each
heartbeat, Scanner + Beacon doubles the scan pulse duration, Exfil + Router lifts
the backpressure ceiling so packets flow freely.

### Factions & personas

Each run starts with 1-4 command-and-control (C2) nodes, each rooting its own
faction. Factions compete over the mesh via:

- **Personas** — every faction rolls one of four AI personalities at spawn
  (`aggressor`, `fortress`, `plague`, `opportunist`) that bias its role weights and
  event-roll tendencies. Personas shift dynamically as a faction's fortunes change
  — a losing faction turtles up, a winning one goes expansion mode.
- **Faction-shuffled palette** — each run draws 12-color palettes per theme and
  shuffles the assignment, so which hue represents which faction varies run to run.
- **Rivalries** — cross-faction kills and worm crossings accumulate per-pair war
  pressure. High pressure amplifies border skirmish and cross-faction bridge
  chances.
- **Wars** — rivalry crossing a threshold declares open war (`⚔`) with a 3× border
  skirmish multiplier for a window.
- **Alliances** — peace treaties that suppress skirmishes and cross-faction bridging
  between the signatories.
- **Assimilation** — a weak faction dropping below the dominance threshold gets
  absorbed by a strong one, its surviving nodes reparented to the nearest strong
  neighbor.
- **Dominance** — when one faction holds ≥ 60% of alive nodes (or is the last C2
  standing) it takes dominance and the header bar shows a live `✦ F{N} dominant`
  badge. Dominance is re-evaluated continuously and can change hands multiple times
  in a long run.
- **C2 siege** — each C2 starts with a 200-point pwn_resist reservoir. Enemy worms
  that reach a C2 drain 8 per hit. At zero, the C2 falls and its whole subtree
  cascades via a `✦ FALL ✦` mythic event.
- **Resurrection** — when a large cascade dooms 10+ nodes, one of them has a 55%
  chance to rise as a fresh C2 with its own persona, palette, and HP — literally
  rising from the ashes.

### Viruses & infection

Infections are a first-class system with their own lifecycle:

- **Strains** — eight named strains, deterministic per seed.
- **Stages** — incubating (stealthy) → active (visible, suppresses role behavior) →
  terminal (crashes the host into Pwned).
- **Ransomware variants** — rare strain subtype that freezes the host indefinitely
  instead of killing it; immune to patch waves, only defender pulses clear.
- **Veteran promotion** — strains that survive patch-wave cures gain permanent
  cure_resist bumps up to a cap, getting harder to clear as they age.
- **Strain merging** — a worm landing on a host infected with a *different* strain
  merges the two into a hybrid with combined resist and a new strain id.
- **Strain ecology** — when multiple strains coexist, the lowest-strength strain
  has a 1%/tick chance of being outcompeted and cleared.
- **Post-cure immunity** — cured nodes gain a 180-tick strain-specific immunity
  window, creating visible "cured pockets" that the prior strain can't re-infect.
- **Antibody worms** — Defender cures probabilistically launch reversed-color `◈`
  worms that travel existing links and cure whatever they arrive at.
- **Hunter cull** — a `⟁` Hunter node periodically exploits adjacent same-faction
  infected neighbors to cut off strain spread at the cost of the host.
- **Zero-day events** — periodic global events: outbreak (mass-seed a strain),
  emergency patch (clear all incubating), or immune breakthrough (boost all
  cure_resist counters).

### Environmental events

- **Day/night cycle** — spawn/loss rates oscillate, background territory dims at
  night.
- **Storms** — a rolling front of bright crackle sweeps across the mesh from the
  top edge; spawn/loss rates spike for the duration.
- **DDoS waves** — a line of hostile traffic sweeps from one edge to the opposite
  one, stunning every node it passes over.
- **Wormholes** — a rare dashed rift flickers between two random alive cells.
- **ISP outages** — rare rectangular dead zones where new spawns are blocked and
  any alive nodes inside get steady role-cooldown stuns.
- **Network partitions** — horizontal or vertical line cuts through the mesh that
  drop packets and worms trying to cross them.
- **Fiber hotspots** — persistent fixed terrain zones rolled at world creation.
  Nodes spawned inside gain bonus pwn_resist and start with a defensive head-start.
- **Sleeper agents** — rare spawns that look like any ordinary role but secretly
  belong to another faction. At a probabilistic trigger they wake, flip faction,
  plant an infection on themselves, and fire a `✦ sleeper ✦` mythic reveal.
- **Legendary promotions** — nodes that survive 1200+ ticks AND spawn 8+ children
  get a random name from a pool and a permanent bold/underlined render highlight.
  Each legendary gets a deterministically-generated one-line bio shown in the
  inspector and the end-of-run summary.

### Traffic & congestion

- **Packets** — fired by exfils on a cooldown, hop parent-chain toward C2. Each
  delivery credits intel and contributes to the run score.
- **Link load** — every in-flight packet heats its carrier link. Hot links refuse
  new packets; warm links trigger exfil backpressure (exfils skip their emission
  cycle and retry on a shorter cooldown).
- **Cross-link rerouting** — when a packet hits a congested leg, it scans adjacent
  cross-links for a cooler alternate route and jumps onto it instead of dropping.
- **Router absorption** — Routers cache ~65% of the packets that reach them,
  letting the rest pass through to C2 so backbones still see traffic.
- **Backbone promotion** — Parent links that deliver enough packets get promoted
  to backbones with an inflated HOT_LINK ceiling and thicker box-drawing glyphs.
- **Link overload collapse** — links that sustain HOT traffic past a threshold
  flush all their packets, stun both endpoints, and quarantine for a window.
  Logs as `⚡ LINK OVERLOAD`.

## Themes

Built-in themes, select with `--theme`:

- `cyberpunk` (default)
- `aretha-dark` — navy + sky blue, based on the Aretha Plasma KDE scheme
- `gruvbox`
- `nord`
- `dracula`
- `catppuccin-mocha`
- `solarized-dark`

Every theme ships a 12-color faction palette, a branch palette, and strain colors
for the infection layer. Drop a custom theme TOML anywhere and pass it by path:

```bash
cargo run -- --theme ./my-theme.toml
```

Themes are merged over the cyberpunk default so you only have to override the
fields you care about.

## Configuration

Every CLI flag has a matching field in a TOML config file. Default location is
`~/.config/netgrow/config.toml`. Pass `--config <path>` to load from elsewhere.
Explicit CLI flags always win over file values.

Key knobs:

| Flag | Default | What it controls |
|------|---------|------------------|
| `--seed <u64>` | random | Deterministic per-run seed |
| `--tick-ms <ms>` | 50 | Sim tick period |
| `--spawn-rate <f>` | 0.15 | New-node spawn probability per tick |
| `--loss-rate <f>` | 0.005 | Random-loss probability per tick |
| `--max-nodes <n>` | 400 | Population cap |
| `--c2-count <n>` | 1 | Minimum starting C2s |
| `--c2-count-max <n>` | 4 | Maximum starting C2s (random in [min..max]) |
| `--day-night-period <t>` | 600 | Length of day/night cycle (0 = off) |
| `--virus-spread-rate <f>` | 0.05 | How aggressively viruses spread |
| `--disable-virus` | false | Kill the entire viral layer |
| `--theme <name\|path>` | cyberpunk | Built-in name or path to a custom TOML |

Run `cargo run -- --help` for the full list — every role weight, event period,
and tunable is exposed.

## End-of-run summary

Press `q` at any time to exit to a full-screen summary with:

- An ASCII banner and dominance callout (winner faction + persona + elapsed time)
- Run info (session name, seed, era, config snippets)
- Totals panel (spawned, lost, cured, traps, intel, shifts, total score)
- Sorted faction leaderboard with medals, scores, score bars, and per-faction stats
- Legends panel listing any still-alive legendary nodes with bios
- "Press any key to disconnect" prompt

## Architecture sketch

```
src/
  main.rs          — CLI, event loop, top-level UI state
  config.rs        — TOML file schema and loader
  theme.rs         — Theme type, builtin themes, file loader
  render.rs        — All rendering, panels, widgets
  routing.rs       — Link path router between two cells
  util.rs          — Braille graph helpers, session name, formatting
  world/
    mod.rs         — World struct, tick loop orchestration, cross-system helpers
    types.rs       — Pure data types (Node, Link, Infection, Hotspot, etc.)
    config.rs      — Config struct + strain/era/legendary name pools
    spawn.rs       — try_spawn, roll_role, maybe_reconnect, link routing
    roles.rs       — Per-role firing: scanners, exfils, defenders, hunters
    packets.rs     — Packet/worm transport, antibody cures, backbone promotion
    virus.rs       — Infection stages, spread, cures, veteran ranks, ecology
    cascade.rs     — Loss resolution, subtree death, honeypot backdoors, resurrection
    events.rs      — Alliances, skirmishes, assimilation, wormholes, DDoS, storms,
                     ISP outages, partitions
    tests.rs       — Integration tests
themes/            — Built-in theme TOML files
```

## Why

Because a terminal is still the best frame for watching something alive grow and
die. netgrow is half simulation toy, half ambient screensaver, half experiment in
what TUI state visualization can actually express. Built during a long weekend
with a lot of help from Claude Code.

## License

MIT or whatever you want. This is a toy.
