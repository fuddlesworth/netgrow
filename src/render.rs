use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph, Widget};
use ratatui::Frame;

use crate::world::{Node, Role, State, World};

pub fn draw(frame: &mut Frame, world: &World) {
    let chunks = Layout::horizontal([Constraint::Min(40), Constraint::Length(28)])
        .split(frame.area());

    let mesh_block = Block::bordered().title(" netgrow ");
    let log_block = Block::bordered().title(" log ");
    let mesh_area = mesh_block.inner(chunks[0]);
    let log_area = log_block.inner(chunks[1]);

    frame.render_widget(mesh_block, chunks[0]);
    frame.render_widget(log_block, chunks[1]);
    frame.render_widget(MeshWidget { world }, mesh_area);

    let log_lines: Vec<Line> = world
        .logs
        .iter()
        .rev()
        .take(log_area.height as usize)
        .map(|s| Line::from(s.as_str()))
        .collect();
    frame.render_widget(Paragraph::new(log_lines), log_area);
}

pub struct MeshWidget<'a> {
    pub world: &'a World,
}

impl<'a> Widget for MeshWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let w = self.world;

        // 1. Links
        for link in &w.links {
            let a = &w.nodes[link.a];
            let b = &w.nodes[link.b];
            let dying = a.dying_in > 0 || b.dying_in > 0;
            let dead = matches!(a.state, State::Dead) || matches!(b.state, State::Dead);
            let style = if dying {
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD)
            } else if dead {
                Style::default().fg(Color::DarkGray)
            } else if matches!(a.state, State::Pwned { .. })
                || matches!(b.state, State::Pwned { .. })
            {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(branch_hue(b.branch_id))
            };
            // Once a link's endpoint is dying or dead, reveal the full route —
            // otherwise a cascade that fires mid-animation leaves red ✕ markers
            // floating with no trail back to their parent.
            let reveal = if dying || dead {
                link.path.len()
            } else {
                (link.drawn as usize).min(link.path.len())
            };
            if reveal == 0 {
                continue;
            }
            for i in 0..reveal {
                let cell = link.path[i];
                if cell == w.nodes[link.a].pos || cell == w.nodes[link.b].pos {
                    continue;
                }
                let prev = if i > 0 { Some(link.path[i - 1]) } else { None };
                let next = if i + 1 < reveal {
                    Some(link.path[i + 1])
                } else {
                    None
                };
                let glyph = glyph_for(prev, cell, next);
                put(buf, area, cell, glyph, style);
            }
        }

        // 2. Scanner ping halos — expand 1/2/3 cells outward per tick of life.
        for ping in &w.pings {
            let age = w.tick.saturating_sub(ping.born) as i16;
            if age > 3 {
                continue;
            }
            let radius = age.max(1);
            let dim = 80u8.saturating_sub((age as u8) * 20);
            let style = Style::default().fg(Color::Rgb(dim, 220, 220));
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    if dx.abs().max(dy.abs()) != radius {
                        continue;
                    }
                    let cell = (ping.origin.0 + dx, ping.origin.1 + dy);
                    put(buf, area, cell, "·", style);
                }
            }
        }

        // 3. Exfil packets flowing back toward C2.
        for pkt in &w.packets {
            let link = &w.links[pkt.link_id];
            let idx = pkt.pos as usize;
            if idx >= link.path.len() {
                continue;
            }
            let cell = link.path[idx];
            // Skip drawing on endpoint cells — nodes take priority.
            if cell == w.nodes[link.a].pos || cell == w.nodes[link.b].pos {
                continue;
            }
            let glyph = packet_glyph(link, idx);
            let style = Style::default()
                .fg(Color::Rgb(120, 240, 255))
                .add_modifier(Modifier::BOLD);
            put(buf, area, cell, glyph, style);
        }

        // 4. Nodes on top.
        for node in &w.nodes {
            let (glyph, style) = node_glyph(node, w.tick);
            put(buf, area, node.pos, glyph, style);
        }
    }
}

fn put(buf: &mut Buffer, area: Rect, pos: (i16, i16), glyph: &str, style: Style) {
    if pos.0 < 0 || pos.1 < 0 {
        return;
    }
    let x = area.x as i32 + pos.0 as i32;
    let y = area.y as i32 + pos.1 as i32;
    if x < area.x as i32 || y < area.y as i32 {
        return;
    }
    if x >= area.right() as i32 || y >= area.bottom() as i32 {
        return;
    }
    if let Some(cell) = buf.cell_mut((x as u16, y as u16)) {
        cell.set_symbol(glyph).set_style(style);
    }
}

fn glyph_for(prev: Option<(i16, i16)>, cur: (i16, i16), next: Option<(i16, i16)>) -> &'static str {
    let dir = |a: (i16, i16), b: (i16, i16)| (b.0 - a.0, b.1 - a.1);
    match (prev, next) {
        (Some(p), Some(n)) => {
            let d1 = dir(p, cur);
            let d2 = dir(cur, n);
            if d1.0 == 0 && d2.0 == 0 {
                "│"
            } else if d1.1 == 0 && d2.1 == 0 {
                "─"
            } else {
                match (d1, d2) {
                    ((1, 0), (0, 1)) | ((0, -1), (-1, 0)) => "┐",
                    ((1, 0), (0, -1)) | ((0, 1), (-1, 0)) => "┘",
                    ((-1, 0), (0, 1)) | ((0, -1), (1, 0)) => "┌",
                    ((-1, 0), (0, -1)) | ((0, 1), (1, 0)) => "└",
                    _ => "·",
                }
            }
        }
        (None, Some(n)) | (Some(n), None) => {
            let d = dir(cur, n);
            if d.0 == 0 {
                "│"
            } else if d.1 == 0 {
                "─"
            } else {
                "·"
            }
        }
        (None, None) => "·",
    }
}

fn packet_glyph(link: &crate::world::Link, idx: usize) -> &'static str {
    // Direction of travel: from idx toward idx-1 (parent end).
    if idx == 0 {
        return "▸";
    }
    let cur = link.path[idx];
    let prev = link.path[idx - 1];
    let dx = prev.0 - cur.0;
    let dy = prev.1 - cur.1;
    match (dx.signum(), dy.signum()) {
        (1, 0) => "▸",
        (-1, 0) => "◂",
        (0, 1) => "▾",
        (0, -1) => "▴",
        _ => "◆",
    }
}

fn branch_hue(branch_id: u16) -> Color {
    const PALETTE: [Color; 8] = [
        Color::Rgb(60, 200, 100),
        Color::Rgb(80, 220, 160),
        Color::Rgb(180, 220, 60),
        Color::Rgb(60, 180, 200),
        Color::Rgb(200, 220, 80),
        Color::Rgb(40, 220, 140),
        Color::Rgb(120, 200, 80),
        Color::Rgb(60, 200, 180),
    ];
    PALETTE[branch_id as usize % PALETTE.len()]
}

fn node_glyph(node: &Node, tick: u64) -> (&'static str, Style) {
    if node.dying_in > 0 && !matches!(node.state, State::Dead) {
        let st = if (tick + node.dying_in as u64) % 2 == 0 {
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD)
        };
        return ("✕", st);
    }
    match node.state {
        State::Alive => {
            if node.parent.is_none() {
                return (
                    "◆",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                );
            }
            // Honeypot revealed at trip moment — yellow flash.
            if node.honey_reveal > 0 {
                return (
                    "◈",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED),
                );
            }
            let hue = branch_hue(node.branch_id);
            let pulse_boost = node.pulse > 0;
            let (glyph, base_style) = match node.role {
                Role::Relay => {
                    if node.hardened {
                        ("◉", Style::default().fg(hue).add_modifier(Modifier::BOLD))
                    } else {
                        ("●", Style::default().fg(hue))
                    }
                }
                Role::Scanner => (
                    "◎",
                    Style::default()
                        .fg(Color::Rgb(120, 220, 255))
                        .add_modifier(if node.hardened { Modifier::BOLD } else { Modifier::empty() }),
                ),
                Role::Exfil => (
                    "▣",
                    Style::default()
                        .fg(Color::Rgb(180, 180, 255))
                        .add_modifier(if node.hardened { Modifier::BOLD } else { Modifier::empty() }),
                ),
                // Honeypot masquerades as a Relay until tripped.
                Role::Honeypot => ("●", Style::default().fg(hue)),
            };
            if pulse_boost {
                (
                    glyph,
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                (glyph, base_style)
            }
        }
        State::Pwned { .. } => {
            let st = if tick % 2 == 0 {
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::LightRed)
            };
            ("✕", st)
        }
        State::Dead => ("·", Style::default().fg(Color::DarkGray)),
    }
}
