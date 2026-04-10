use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph, Widget};
use ratatui::Frame;

use crate::world::{Link, Node, State, World};

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

        for link in &w.links {
            let a = &w.nodes[link.a];
            let b = &w.nodes[link.b];
            let dying = a.dying_in > 0 || b.dying_in > 0;
            let dead = matches!(a.state, State::Dead) || matches!(b.state, State::Dead);
            let style = if dying {
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD)
            } else {
                link_style(a.state, b.state)
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
                // Skip the endpoint cells; nodes draw on top of them.
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
                // Bend: figure out which corner.
                // Incoming direction and outgoing direction.
                match (d1, d2) {
                    // came from left (+x), going down (+y): ┐
                    ((1, 0), (0, 1)) | ((0, -1), (-1, 0)) => "┐",
                    // came from left (+x), going up (-y): ┘
                    ((1, 0), (0, -1)) | ((0, 1), (-1, 0)) => "┘",
                    // came from right (-x), going down (+y): ┌
                    ((-1, 0), (0, 1)) | ((0, -1), (1, 0)) => "┌",
                    // came from right (-x), going up (-y): └
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

fn link_style(a: State, b: State) -> Style {
    let any_dead = matches!(a, State::Dead) || matches!(b, State::Dead);
    let any_pwned = matches!(a, State::Pwned { .. }) || matches!(b, State::Pwned { .. });
    if any_dead {
        Style::default().fg(Color::DarkGray)
    } else if any_pwned {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::Rgb(60, 200, 100))
    }
}

fn node_glyph(node: &Node, tick: u64) -> (&'static str, Style) {
    if node.dying_in > 0 && !matches!(node.state, State::Dead) {
        // Blink red ✕ while the death wave is passing through this node.
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
                (
                    "◆",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
            } else if node.pulse > 0 {
                (
                    "●",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                ("●", Style::default().fg(Color::LightGreen))
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

// Keep unused-import silencer: Link referenced for intent.
#[allow(dead_code)]
fn _ref_link(_l: &Link) {}
