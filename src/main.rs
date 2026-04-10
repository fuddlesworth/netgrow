mod config;
mod render;
mod routing;
mod theme;
mod world;

use std::io::{self, stdout, Stdout};
use std::path::PathBuf;
use std::time::Duration;

use clap::{ArgMatches, CommandFactory, FromArgMatches, Parser};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::config::FileConfig;
use crate::render::UiState;
use crate::world::{Config, RoleWeights, World};

#[derive(Parser, Debug)]
#[command(name = "netgrow", about = "A cyberpunk botnet growing in your terminal")]
struct Cli {
    /// Path to a TOML config file. Defaults to
    /// $HOME/.config/netgrow/config.toml when omitted; missing files are
    /// silently ignored, parse errors abort startup.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Theme to apply. Either a built-in name (gruvbox, nord, dracula,
    /// catppuccin-mocha, solarized-dark) or a path to a custom TOML
    /// theme file. Overrides any `theme = ...` in the config file.
    #[arg(long)]
    theme: Option<String>,
    #[arg(long)]
    seed: Option<u64>,
    #[arg(long, default_value_t = 50)]
    tick_ms: u64,
    #[arg(long, default_value_t = 0.15)]
    spawn_rate: f32,
    #[arg(long, default_value_t = 0.005)]
    loss_rate: f32,
    #[arg(long, default_value_t = 400)]
    max_nodes: usize,

    /// Relative weight of Relay nodes at spawn.
    #[arg(long, default_value_t = 0.72)]
    relay_weight: f32,
    /// Relative weight of Scanner nodes at spawn.
    #[arg(long, default_value_t = 0.15)]
    scanner_weight: f32,
    /// Relative weight of Exfil nodes at spawn.
    #[arg(long, default_value_t = 0.10)]
    exfil_weight: f32,
    /// Relative weight of Honeypot nodes at spawn.
    #[arg(long, default_value_t = 0.04)]
    honeypot_weight: f32,
    /// Relative weight of Defender nodes at spawn.
    #[arg(long, default_value_t = 0.08)]
    defender_weight: f32,
    /// Relative weight of Tower nodes at spawn. Towers only materialize
    /// near C2 (within tower_spawn_radius) and absorb extra pwn attempts.
    #[arg(long, default_value_t = 0.05)]
    tower_weight: f32,

    /// Ticks between Scanner pings.
    #[arg(long, default_value_t = 30)]
    scanner_ping_period: u16,
    /// Ticks between Exfil packet emissions.
    #[arg(long, default_value_t = 25)]
    exfil_packet_period: u16,
    /// Heartbeat survivals required before a node hardens.
    #[arg(long, default_value_t = 4)]
    hardened_after: u8,
    /// Multiplier applied to a honeypot's cascade delay for theatrical effect.
    #[arg(long, default_value_t = 3.0)]
    honeypot_cascade_mult: f32,
    /// Per-tick probability of attempting a lateral bridge between two live
    /// nodes in different branches (0 disables the feature).
    #[arg(long, default_value_t = 0.0)]
    reconnect_rate: f32,
    /// Maximum Chebyshev distance between reconnect candidates.
    #[arg(long, default_value_t = 10)]
    reconnect_radius: i16,

    /// Virus spread probability per infected neighbor per tick.
    #[arg(long, default_value_t = 0.05)]
    virus_spread_rate: f32,
    /// Per-tick probability that a mature live node mutates its role.
    #[arg(long, default_value_t = 0.0008)]
    mutate_rate: f32,
    /// Chance that a zero-day event fires when its period elapses.
    #[arg(long, default_value_t = 0.4)]
    zero_day_chance: f32,
    /// Disable the entire virus layer (overrides spread/seed/worm rates).
    #[arg(long, default_value_t = false)]
    disable_virus: bool,
    /// Constant weight given to C2 in parent selection. Higher values create
    /// more distinct branches by keeping C2 a viable parent throughout the
    /// run instead of letting age decay collapse its weight to zero.
    #[arg(long, default_value_t = 0.6)]
    c2_spawn_bias: f32,
    /// Per-spawn chance that a new node forks off into its own branch
    /// instead of inheriting its parent's branch_id. Set to 0 to keep all
    /// branches rooted at C2.
    #[arg(long, default_value_t = 0.05)]
    fork_rate: f32,
    /// Number of C2 nodes to spawn at the start. 1 = single botnet,
    /// 2+ = competing factions, each with its own faction color and
    /// independent reachability cascade.
    #[arg(long, default_value_t = 2)]
    c2_count: u8,
    /// Ticks per full day/night cycle. Spawn and loss rates oscillate
    /// across this period, creating visible waves of activity. Set to 0
    /// to disable the effect entirely.
    #[arg(long, default_value_t = 600)]
    day_night_period: u64,
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen, Hide)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen, Show);
    }
}

/// True iff the named CLI flag came from the command line, not from a
/// clap default. Used by the file-config merge to decide whether the
/// CLI value should override a file value.
fn was_set(matches: &ArgMatches, name: &str) -> bool {
    matches!(
        matches.value_source(name),
        Some(clap::parser::ValueSource::CommandLine)
    )
}

/// `cli` always wins when the user explicitly typed it; otherwise the
/// file value wins if present; otherwise the clap default (already
/// inside `cli`) is used.
fn pick<T: Copy>(matches: &ArgMatches, name: &str, cli_val: T, file_val: Option<T>) -> T {
    if was_set(matches, name) {
        cli_val
    } else {
        file_val.unwrap_or(cli_val)
    }
}

fn main() -> io::Result<()> {
    let matches = Cli::command().get_matches();
    let cli = Cli::from_arg_matches(&matches).expect("clap parse");

    // Resolve the config file path: explicit --config, else conventional
    // XDG location. Missing files are silently treated as "no overrides".
    let file_path = cli
        .config
        .clone()
        .or_else(FileConfig::default_path);
    let file = match file_path.as_ref() {
        Some(p) => FileConfig::load(p)?,
        None => FileConfig::default(),
    };
    if let Some(p) = file_path.as_ref() {
        if p.exists() {
            eprintln!("netgrow loaded config from {}", p.display());
        }
    }

    // Theme: --theme on the CLI wins over `theme = "..."` in the config
    // file. Both accept either a built-in name (nord, dracula, …) or a
    // filesystem path. Relative paths from the config file resolve
    // against that file's directory; relative paths from the CLI resolve
    // against the current working directory.
    let theme_request = cli.theme.as_deref().or(file.theme.as_deref());
    if let Some(req) = theme_request {
        let base_dir = if cli.theme.is_some() {
            std::env::current_dir().ok()
        } else {
            file_path
                .as_ref()
                .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        };
        match theme::Theme::resolve(req, base_dir.as_deref()) {
            Ok(t) => {
                eprintln!("netgrow loaded theme '{}'", req);
                theme::install(t);
            }
            Err(e) => {
                eprintln!("netgrow theme load failed: {}", e);
                eprintln!("built-in themes: {}", theme::BUILTIN_NAMES.join(", "));
                return Err(e);
            }
        }
    }

    // Field-by-field merge: explicit CLI flags > file values > clap defaults.
    let pick_u64 =
        |name: &str, cli_val: u64, file_val: Option<u64>| pick(&matches, name, cli_val, file_val);
    let pick_f32 =
        |name: &str, cli_val: f32, file_val: Option<f32>| pick(&matches, name, cli_val, file_val);
    let pick_u16 =
        |name: &str, cli_val: u16, file_val: Option<u16>| pick(&matches, name, cli_val, file_val);
    let pick_u8 =
        |name: &str, cli_val: u8, file_val: Option<u8>| pick(&matches, name, cli_val, file_val);
    let pick_i16 =
        |name: &str, cli_val: i16, file_val: Option<i16>| pick(&matches, name, cli_val, file_val);
    let pick_usize = |name: &str, cli_val: usize, file_val: Option<usize>| {
        pick(&matches, name, cli_val, file_val)
    };
    let pick_bool = |name: &str, cli_val: bool, file_val: Option<bool>| {
        pick(&matches, name, cli_val, file_val)
    };

    let tick_ms = pick_u64("tick_ms", cli.tick_ms, file.tick_ms);
    let spawn_rate = pick_f32("spawn_rate", cli.spawn_rate, file.spawn_rate);
    let loss_rate = pick_f32("loss_rate", cli.loss_rate, file.loss_rate);
    let max_nodes = pick_usize("max_nodes", cli.max_nodes, file.max_nodes);
    let relay_weight = pick_f32("relay_weight", cli.relay_weight, file.relay_weight);
    let scanner_weight = pick_f32("scanner_weight", cli.scanner_weight, file.scanner_weight);
    let exfil_weight = pick_f32("exfil_weight", cli.exfil_weight, file.exfil_weight);
    let honeypot_weight = pick_f32("honeypot_weight", cli.honeypot_weight, file.honeypot_weight);
    let defender_weight = pick_f32("defender_weight", cli.defender_weight, file.defender_weight);
    let tower_weight = pick_f32("tower_weight", cli.tower_weight, file.tower_weight);
    let scanner_ping_period = pick_u16(
        "scanner_ping_period",
        cli.scanner_ping_period,
        file.scanner_ping_period,
    );
    let exfil_packet_period = pick_u16(
        "exfil_packet_period",
        cli.exfil_packet_period,
        file.exfil_packet_period,
    );
    let hardened_after = pick_u8("hardened_after", cli.hardened_after, file.hardened_after);
    let honeypot_cascade_mult = pick_f32(
        "honeypot_cascade_mult",
        cli.honeypot_cascade_mult,
        file.honeypot_cascade_mult,
    );
    let reconnect_rate = pick_f32("reconnect_rate", cli.reconnect_rate, file.reconnect_rate);
    let reconnect_radius = pick_i16(
        "reconnect_radius",
        cli.reconnect_radius,
        file.reconnect_radius,
    );
    let virus_spread_rate = pick_f32(
        "virus_spread_rate",
        cli.virus_spread_rate,
        file.virus_spread_rate,
    );
    let mutate_rate = pick_f32("mutate_rate", cli.mutate_rate, file.mutate_rate);
    let zero_day_chance = pick_f32("zero_day_chance", cli.zero_day_chance, file.zero_day_chance);
    let disable_virus = pick_bool("disable_virus", cli.disable_virus, file.disable_virus);
    let c2_spawn_bias = pick_f32("c2_spawn_bias", cli.c2_spawn_bias, file.c2_spawn_bias);
    let fork_rate = pick_f32("fork_rate", cli.fork_rate, file.fork_rate);
    let c2_count = pick_u8("c2_count", cli.c2_count, file.c2_count);
    let day_night_period = pick_u64(
        "day_night_period",
        cli.day_night_period,
        file.day_night_period,
    );

    let seed = cli.seed.or(file.seed).unwrap_or_else(rand::random);
    eprintln!("netgrow seed = {}", seed);

    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal: Terminal<CrosstermBackend<Stdout>> = Terminal::new(backend)?;

    let size = terminal.size()?;
    let initial_bounds = render::mesh_bounds(size);
    let mut cfg = Config {
        p_spawn: spawn_rate,
        p_loss: loss_rate,
        max_nodes,
        role_weights: RoleWeights {
            relay: relay_weight,
            scanner: scanner_weight,
            exfil: exfil_weight,
            honeypot: honeypot_weight,
            defender: defender_weight,
            tower: tower_weight,
        },
        scanner_ping_period,
        exfil_packet_period,
        hardened_after_heartbeats: hardened_after,
        honeypot_cascade_mult,
        reconnect_rate,
        reconnect_radius,
        virus_spread_rate,
        mutate_rate,
        zero_day_chance,
        c2_spawn_bias,
        fork_rate,
        c2_count,
        day_night_period,
        ..Config::default()
    };
    if disable_virus {
        cfg.virus_spread_rate = 0.0;
        cfg.virus_seed_rate = 0.0;
        cfg.worm_spawn_rate = 0.0;
        cfg.zero_day_chance = 0.0;
    }
    let mut world = World::new(seed, initial_bounds, cfg);

    let mut tick_ms = tick_ms;
    let mut paused = false;
    let mut mesh_bounds = initial_bounds;
    let mut cursor: Option<(i16, i16)> = None;

    loop {
        let wait = Duration::from_millis(tick_ms);
        if event::poll(wait)? {
            if let Event::Key(KeyEvent {
                code, modifiers, ..
            }) = event::read()?
            {
                match (code, modifiers) {
                    (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => break,
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => break,
                    (KeyCode::Char(' '), _) => paused = !paused,
                    (KeyCode::Char('+'), _) | (KeyCode::Char('='), _) => {
                        tick_ms = tick_ms.saturating_sub(10).max(10);
                    }
                    (KeyCode::Char('-'), _) | (KeyCode::Char('_'), _) => {
                        tick_ms = (tick_ms + 10).min(500);
                    }
                    (KeyCode::Char('i'), _) => {
                        world.inject_infection();
                    }
                    (KeyCode::Tab, _) => {
                        cursor = if cursor.is_some() {
                            None
                        } else {
                            Some((mesh_bounds.0 / 2, mesh_bounds.1 / 2))
                        };
                    }
                    (KeyCode::Left, _) if cursor.is_some() => {
                        if let Some(c) = cursor.as_mut() {
                            c.0 = (c.0 - 1).max(0);
                        }
                    }
                    (KeyCode::Right, _) if cursor.is_some() => {
                        if let Some(c) = cursor.as_mut() {
                            c.0 = (c.0 + 1).min(mesh_bounds.0 - 1);
                        }
                    }
                    (KeyCode::Up, _) if cursor.is_some() => {
                        if let Some(c) = cursor.as_mut() {
                            c.1 = (c.1 - 1).max(0);
                        }
                    }
                    (KeyCode::Down, _) if cursor.is_some() => {
                        if let Some(c) = cursor.as_mut() {
                            c.1 = (c.1 + 1).min(mesh_bounds.1 - 1);
                        }
                    }
                    _ => {}
                }
            }
            // Redraw immediately so key feedback (pause, speed) is visible.
            let ui = UiState {
                paused,
                tick_ms,
                seed,
                cursor,
            };
            terminal.draw(|f| {
                render::draw(f, &world, ui);
            })?;
            continue;
        }

        if !paused {
            world.tick(mesh_bounds);
        }

        let ui = UiState {
            paused,
            tick_ms,
            seed,
            cursor,
        };
        terminal.draw(|f| {
            render::draw(f, &world, ui);
        })?;

        mesh_bounds = render::mesh_bounds(terminal.size()?);
    }

    // Drop the terminal guard before printing the session summary so
    // the alternate screen exits and the text lands in the user's
    // normal scrollback. `_guard` going out of scope here handles that.
    drop(_guard);
    eprintln!("\nnetgrow session summary  seed={}", seed);
    eprintln!("ticks  {}", world.tick);
    eprintln!("era    {}", world.epoch_name());
    for (i, fs) in world.faction_stats.iter().enumerate() {
        eprintln!(
            "F{}  score {:+}  spawned {}  lost {}  traps {}  cured {}",
            i, fs.score(), fs.spawned, fs.lost, fs.honeys_tripped, fs.infections_cured
        );
    }

    Ok(())
}
