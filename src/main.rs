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

use crate::world::{Config, World};

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
