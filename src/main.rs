mod render;
mod routing;
mod world;

use std::io::{self, stdout, Stdout};
use std::time::Duration;

use clap::Parser;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::render::UiState;
use crate::world::{Config, RoleWeights, World};

#[derive(Parser, Debug)]
#[command(name = "netgrow", about = "A cyberpunk botnet growing in your terminal")]
struct Cli {
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
    #[arg(long, default_value_t = 0.03)]
    honeypot_weight: f32,

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

fn main() -> io::Result<()> {
    let cli = Cli::parse();
    let seed = cli.seed.unwrap_or_else(rand::random);
    eprintln!("netgrow seed = {}", seed);

    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal: Terminal<CrosstermBackend<Stdout>> = Terminal::new(backend)?;

    let size = terminal.size()?;
    let initial_bounds = render::mesh_bounds(size);
    let cfg = Config {
        p_spawn: cli.spawn_rate,
        p_loss: cli.loss_rate,
        max_nodes: cli.max_nodes,
        role_weights: RoleWeights {
            relay: cli.relay_weight,
            scanner: cli.scanner_weight,
            exfil: cli.exfil_weight,
            honeypot: cli.honeypot_weight,
        },
        scanner_ping_period: cli.scanner_ping_period,
        exfil_packet_period: cli.exfil_packet_period,
        hardened_after_heartbeats: cli.hardened_after,
        honeypot_cascade_mult: cli.honeypot_cascade_mult,
        reconnect_rate: cli.reconnect_rate,
        reconnect_radius: cli.reconnect_radius,
        virus_spread_rate: if cli.disable_virus { 0.0 } else { cli.virus_spread_rate },
        virus_seed_rate: if cli.disable_virus { 0.0 } else { 0.004 },
        worm_spawn_rate: if cli.disable_virus { 0.0 } else { 0.04 },
        mutate_rate: cli.mutate_rate,
        zero_day_chance: if cli.disable_virus { 0.0 } else { cli.zero_day_chance },
        ..Config::default()
    };
    let mut world = World::new(seed, initial_bounds, cfg);

    let mut tick_ms: u64 = cli.tick_ms;
    let mut paused = false;
    let mut mesh_bounds = initial_bounds;

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
                    _ => {}
                }
            }
            // Redraw immediately so key feedback (pause, speed) is visible.
            let ui = UiState { paused, tick_ms, seed };
            terminal.draw(|f| {
                render::draw(f, &world, ui);
            })?;
            continue;
        }

        if !paused {
            world.tick(mesh_bounds);
        }

        let ui = UiState { paused, tick_ms, seed };
        terminal.draw(|f| {
            render::draw(f, &world, ui);
        })?;

        mesh_bounds = render::mesh_bounds(terminal.size()?);
    }

    Ok(())
}
