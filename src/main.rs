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
    let initial_bounds = (
        (size.width.saturating_sub(30)).saturating_sub(2) as i16,
        size.height.saturating_sub(2) as i16,
    );
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
        ..Config::default()
    };
    let mut world = World::new(seed, initial_bounds, cfg);

    let tick_dur = Duration::from_millis(cli.tick_ms);
    let mut mesh_bounds = initial_bounds;

    loop {
        if event::poll(tick_dur)? {
            if let Event::Key(KeyEvent {
                code, modifiers, ..
            }) = event::read()?
            {
                match (code, modifiers) {
                    (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => break,
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => break,
                    _ => {}
                }
            }
            continue;
        }

        world.tick(mesh_bounds);

        terminal.draw(|f| {
            render::draw(f, &world);
        })?;

        let s = terminal.size()?;
        mesh_bounds = (
            (s.width.saturating_sub(30)).saturating_sub(2) as i16,
            s.height.saturating_sub(2) as i16,
        );
    }

    Ok(())
}
