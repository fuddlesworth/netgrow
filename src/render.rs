use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Layout, Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Widget};
use ratatui::Frame;

use crate::theme::theme;
use crate::world::{InfectionStage, LinkKind, Node, Role, State, World, WorldStats};

const RIGHT_COL_WIDTH: u16 = 42;
const HEADER_HEIGHT: u16 = 1;
const FOOTER_HEIGHT: u16 = 1;

#[derive(Clone, Copy)]
pub struct UiState {
    pub paused: bool,
    pub tick_ms: u64,
    pub seed: u64,
    /// When `Some`, draws an inspector cursor highlight at the given mesh
    /// cell and shows an inspector panel with the node's details.
    pub cursor: Option<(i16, i16)>,
}

pub fn mesh_bounds(size: Size) -> (i16, i16) {
    // Mirror the layout below so the world sizes its spawn area correctly.
    let w = size.width.saturating_sub(RIGHT_COL_WIDTH).saturating_sub(2);
    let h = size
        .height
        .saturating_sub(HEADER_HEIGHT + FOOTER_HEIGHT)
        .saturating_sub(2);
    (w as i16, h as i16)
}

pub fn draw(frame: &mut Frame, world: &World, ui: UiState) {
    let area = frame.area();
    let stats = world.stats();

    let rows = Layout::vertical([
        Constraint::Length(HEADER_HEIGHT),
        Constraint::Min(5),
        Constraint::Length(FOOTER_HEIGHT),
    ])
    .split(area);

    let header_area = rows[0];
    let main_area = rows[1];
    let footer_area = rows[2];

    frame.render_widget(header_bar(world, &stats, ui), header_area);
    frame.render_widget(footer_bar(ui), footer_area);

    let cols = Layout::horizontal([
        Constraint::Min(40),
        Constraint::Length(RIGHT_COL_WIDTH),
    ])
    .split(main_area);

    let mesh_frame = cols[0];
    let right_col = cols[1];

    let mesh_block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme().frame))
        .title(Span::styled(
            " netgrow ",
            Style::default()
                .fg(theme().frame_accent)
                .add_modifier(Modifier::BOLD),
        ));
    let mesh_inner = mesh_block.inner(mesh_frame);
    frame.render_widget(mesh_block, mesh_frame);
    frame.render_widget(
        MeshWidget {
            world,
            cursor: ui.cursor,
        },
        mesh_inner,
    );

    let inspector_height: u16 = if ui.cursor.is_some() { 10 } else { 0 };
    let right_rows = Layout::vertical([
        Constraint::Length(8),
        Constraint::Length(11),
        Constraint::Length(inspector_height),
        Constraint::Min(5),
    ])
    .split(right_col);

    frame.render_widget(stats_block(&stats), right_rows[0]);
    frame.render_widget(legend_block(), right_rows[1]);
    if let Some(pos) = ui.cursor {
        frame.render_widget(inspector_block(world, pos), right_rows[2]);
    }
    frame.render_widget(log_block(world), right_rows[3]);
}

fn header_bar(world: &World, stats: &WorldStats, ui: UiState) -> Paragraph<'static> {
    let th = theme();
    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled(
        " netgrow ",
        Style::default()
            .fg(th.header_brand_fg)
            .bg(th.header_brand_bg)
            .add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        format!("t={}", world.tick),
        Style::default().fg(th.label),
    ));
    spans.push(sep());
    spans.push(stat_span("nodes", format!("{}", stats.alive + stats.pwned)));
    if stats.factions > 1 {
        spans.push(sep());
        spans.push(stat_span("factions", format!("{}", stats.factions)));
    }
    spans.push(sep());
    spans.push(stat_span("branches", format!("{}", stats.branches)));
    spans.push(sep());
    spans.push(stat_span("links", format!("{}", stats.links)));
    if stats.cross_links > 0 {
        spans.push(Span::raw("/"));
        spans.push(Span::styled(
            format!("{}x", stats.cross_links),
            Style::default().fg(th.cross_link),
        ));
    }
    if stats.dying > 0 {
        spans.push(sep());
        spans.push(Span::styled(
            format!("dying {}", stats.dying),
            Style::default().fg(th.pwned).add_modifier(Modifier::BOLD),
        ));
    }
    if stats.packets > 0 {
        spans.push(sep());
        spans.push(Span::styled(
            format!("pkts {}", stats.packets),
            Style::default().fg(th.stat_packets),
        ));
    }
    if stats.infected > 0 {
        spans.push(sep());
        spans.push(Span::styled(
            format!("inf {}", stats.infected),
            Style::default()
                .fg(th.stat_infected)
                .add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(sep());
    spans.push(Span::styled(
        format!("seed {}", ui.seed),
        Style::default().fg(th.ghost),
    ));
    if ui.paused {
        spans.push(sep());
        spans.push(Span::styled(
            " PAUSED ",
            Style::default()
                .fg(th.header_brand_fg)
                .bg(th.honey_reveal)
                .add_modifier(Modifier::BOLD),
        ));
    }
    Paragraph::new(Line::from(spans)).style(Style::default().bg(th.bar_bg))
}

fn footer_bar(ui: UiState) -> Paragraph<'static> {
    let th = theme();
    let key_bg = th.frame; // softer than the brand bg, reads as a key cap
    let key = move |k: &'static str| {
        Span::styled(
            format!(" {} ", k),
            Style::default()
                .fg(th.header_brand_fg)
                .bg(key_bg)
                .add_modifier(Modifier::BOLD),
        )
    };
    let lab = move |t: &'static str| Span::styled(t, Style::default().fg(th.label));
    let spans: Vec<Span<'static>> = vec![
        Span::raw(" "),
        key("q"),
        lab(" quit "),
        key("␣"),
        lab(" pause "),
        key("+"),
        key("-"),
        lab(" speed "),
        key("i"),
        lab(" infect "),
        key("⇥"),
        lab(" inspect "),
        Span::raw(" "),
        Span::styled(
            format!("{}ms/tick", ui.tick_ms),
            Style::default().fg(th.ghost),
        ),
    ];
    Paragraph::new(Line::from(spans))
        .style(Style::default().bg(th.bar_bg))
        .alignment(Alignment::Left)
}

fn stat_span(label: &'static str, value: String) -> Span<'static> {
    Span::styled(
        format!("{} {}", label, value),
        Style::default().fg(theme().stat_value),
    )
}

fn sep() -> Span<'static> {
    Span::styled(" · ", Style::default().fg(theme().ghost))
}

fn stats_block(s: &WorldStats) -> Paragraph<'static> {
    let th = theme();
    let block = bordered_block(" stats ");
    let line = |label: &'static str, value: String, color: Color| {
        Line::from(vec![
            Span::styled(
                format!(" {:<8}", label),
                Style::default().fg(th.stat_label),
            ),
            Span::styled(value, Style::default().fg(color).add_modifier(Modifier::BOLD)),
        ])
    };
    let alive_color = th.branch_palette.first().copied().unwrap_or(th.value);
    let branch_color = th.branch_palette.get(1).copied().unwrap_or(th.value);
    let lines = vec![
        line("alive", format!("{}", s.alive), alive_color),
        line("pwned", format!("{}", s.pwned), th.pwned),
        line("dying", format!("{}", s.dying), th.log_cascade),
        line("dead", format!("{}", s.dead), th.ghost),
        line("branches", format!("{}", s.branches), branch_color),
        line("bridges", format!("{}", s.cross_links), th.cross_link),
    ];
    Paragraph::new(lines).block(block)
}

fn legend_block() -> Paragraph<'static> {
    let th = theme();
    let block = bordered_block(" roles ");
    let row = |glyph: &'static str, glyph_color: Color, name: &'static str| {
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                glyph,
                Style::default().fg(glyph_color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(name, Style::default().fg(th.label)),
        ])
    };
    let relay_color = th.branch_palette.first().copied().unwrap_or(th.value);
    // Note: honeypots are intentionally absent — they masquerade as relays
    // (●) until tripped, at which point ◈ flashes for 2 ticks only. Same
    // reason worms and patch waves aren't here: transient-only glyphs.
    let lines = vec![
        row("◆", faction_hue(0), "c2"),
        row("●", relay_color, "relay"),
        row("◉", relay_color, "hardened"),
        row("◎", th.scanner, "scanner"),
        row("▣", th.exfil, "exfil"),
        row("◇", th.defender, "defender"),
        row("▓", strain_hue(0), "infected"),
        row("✕", th.pwned, "pwned"),
        row("·", th.ghost, "ghost"),
    ];
    Paragraph::new(lines).block(block)
}

fn inspector_block(world: &World, pos: (i16, i16)) -> Paragraph<'static> {
    let th = theme();
    let block = bordered_block(" inspect ");
    let label_style = Style::default().fg(th.stat_label);
    let value_style = Style::default().fg(th.value).add_modifier(Modifier::BOLD);
    let row = |label: &'static str, value: String| {
        Line::from(vec![
            Span::styled(format!(" {:<8}", label), label_style),
            Span::styled(value, value_style),
        ])
    };
    let header = Line::from(vec![
        Span::styled(" cell ", label_style),
        Span::styled(
            format!("{},{}", pos.0, pos.1),
            Style::default().fg(th.cursor).add_modifier(Modifier::BOLD),
        ),
    ]);
    let mut lines: Vec<Line<'static>> = vec![header];
    let node = world.nodes.iter().find(|n| n.pos == pos);
    match node {
        None => {
            lines.push(Line::from(Span::styled(
                " (empty cell)".to_string(),
                Style::default().fg(theme().ghost),
            )));
        }
        Some(n) => {
            let role_name = if n.parent.is_none() {
                "C2"
            } else {
                match n.role {
                    Role::Relay => "relay",
                    Role::Scanner => "scanner",
                    Role::Exfil => "exfil",
                    Role::Honeypot => "honeypot",
                    Role::Defender => "defender",
                }
            };
            lines.push(row("role", role_name.to_string()));
            let state_name = match n.state {
                State::Alive => "alive".to_string(),
                State::Pwned { ticks_left } => format!("pwned ({}t)", ticks_left),
                State::Dead => "dead".to_string(),
            };
            lines.push(row("state", state_name));
            lines.push(row("faction", format!("{}", n.faction)));
            lines.push(row("branch", format!("{}", n.branch_id)));
            let age = world.tick.saturating_sub(n.born);
            lines.push(row("age", format!("{}t", age)));
            let mut tags: Vec<String> = Vec::new();
            if n.hardened {
                tags.push("hardened".into());
            }
            if n.dying_in > 0 {
                tags.push(format!("dying({}t)", n.dying_in));
            }
            if let Some(inf) = n.infection {
                let stage = match inf.stage {
                    InfectionStage::Incubating => "incubating",
                    InfectionStage::Active => "active",
                    InfectionStage::Terminal => "terminal",
                };
                tags.push(format!("strain {} {}", inf.strain, stage));
            }
            let tag_text = if tags.is_empty() {
                "—".to_string()
            } else {
                tags.join(" · ")
            };
            lines.push(row("flags", tag_text));
        }
    }
    Paragraph::new(lines).block(block)
}

fn log_block(world: &World) -> Paragraph<'static> {
    let block = bordered_block(" logs ");
    let lines: Vec<Line<'static>> = world
        .logs
        .iter()
        .rev()
        .take(64)
        .map(|s| color_log_line(s))
        .collect();
    Paragraph::new(lines).block(block)
}

fn bordered_block(title: &'static str) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme().frame))
        .title(Span::styled(
            title,
            Style::default()
                .fg(theme().frame_accent)
                .add_modifier(Modifier::BOLD),
        ))
}

fn color_log_line(s: &str) -> Line<'static> {
    let th = theme();
    let style = if s.contains("HONEYPOT") {
        Style::default()
            .fg(th.header_brand_fg)
            .bg(th.log_honeypot_bg)
            .add_modifier(Modifier::BOLD)
    } else if s.contains("INJECTED") {
        Style::default()
            .fg(th.header_brand_fg)
            .bg(th.log_injected_bg)
            .add_modifier(Modifier::BOLD)
    } else if s.contains("necrotic") {
        Style::default().fg(th.log_strain).add_modifier(Modifier::BOLD)
    } else if s.contains("symptomatic") {
        Style::default().fg(th.log_worm).add_modifier(Modifier::BOLD)
    } else if s.starts_with("strain") {
        Style::default().fg(th.log_strain)
    } else if s.contains("cured") || s.contains("patched") {
        Style::default().fg(th.log_cured).add_modifier(Modifier::BOLD)
    } else if s.starts_with("worm delivered") || s.starts_with("worm launched") {
        Style::default().fg(th.log_worm)
    } else if s.contains("ZERO-DAY") {
        Style::default()
            .fg(th.header_brand_fg)
            .bg(th.log_zero_day_bg)
            .add_modifier(Modifier::BOLD)
    } else if s.contains("mutated") {
        Style::default()
            .fg(th.log_mutated)
            .add_modifier(Modifier::BOLD)
    } else if s.contains("LOST") {
        Style::default().fg(th.log_lost).add_modifier(Modifier::BOLD)
    } else if s.starts_with("cascade") || s.contains("subtree") {
        Style::default()
            .fg(th.log_cascade)
            .add_modifier(Modifier::BOLD)
    } else if s.contains("hardened") {
        Style::default()
            .fg(th.log_hardened)
            .add_modifier(Modifier::BOLD)
    } else if s.contains("shielded") {
        Style::default()
            .fg(th.log_shielded)
            .add_modifier(Modifier::BOLD)
    } else if s.starts_with("bridge") {
        Style::default().fg(th.log_bridge)
    } else if s.starts_with("handshake") {
        Style::default().fg(th.log_handshake)
    } else if s.starts_with("beacon") {
        Style::default().fg(th.log_beacon)
    } else if s.starts_with("c2") {
        Style::default()
            .fg(th.log_c2_online)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(th.log_default)
    };
    Line::from(Span::styled(s.to_string(), style))
}

pub struct MeshWidget<'a> {
    pub world: &'a World,
    pub cursor: Option<(i16, i16)>,
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
            let th = theme();
            let style = if dying {
                Style::default()
                    .fg(th.pwned)
                    .add_modifier(Modifier::BOLD)
            } else if dead {
                Style::default().fg(th.ghost)
            } else if matches!(a.state, State::Pwned { .. })
                || matches!(b.state, State::Pwned { .. })
            {
                Style::default().fg(th.pwned)
            } else if link.kind == LinkKind::Cross {
                Style::default()
                    .fg(th.cross_link)
                    .add_modifier(Modifier::DIM)
            } else {
                Style::default().fg(branch_hue(b.branch_id))
            };
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

        // 1b. C2 patch waves — expanding cure rings from heartbeat sweeps.
        for wave in &w.patch_waves {
            let r = wave.radius;
            if r <= 0 {
                continue;
            }
            let style = Style::default()
                .fg(theme().patch_wave)
                .add_modifier(Modifier::BOLD);
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs().max(dy.abs()) != r {
                        continue;
                    }
                    let cell = (wave.origin.0 + dx, wave.origin.1 + dy);
                    put(buf, area, cell, "○", style);
                }
            }
        }

        // 2. Scanner ping halos
        for ping in &w.pings {
            let age = w.tick.saturating_sub(ping.born) as i16;
            if age > 3 {
                continue;
            }
            let radius = age.max(1);
            let style = Style::default().fg(theme().ping);
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

        // 3. Exfil packets
        for pkt in &w.packets {
            let link = &w.links[pkt.link_id];
            let idx = pkt.pos as usize;
            if idx >= link.path.len() {
                continue;
            }
            let cell = link.path[idx];
            if cell == w.nodes[link.a].pos || cell == w.nodes[link.b].pos {
                continue;
            }
            let glyph = packet_glyph(link, idx);
            let style = Style::default()
                .fg(theme().packet)
                .add_modifier(Modifier::BOLD);
            put(buf, area, cell, glyph, style);
        }

        // 3b. Virus worms crawling along link paths — distinct magenta squares.
        for worm in &w.worms {
            let link = &w.links[worm.link_id];
            let idx = worm.pos as usize;
            if idx >= link.path.len() {
                continue;
            }
            let cell = link.path[idx];
            if cell == w.nodes[link.a].pos || cell == w.nodes[link.b].pos {
                continue;
            }
            let style = Style::default()
                .fg(strain_hue(worm.strain))
                .add_modifier(Modifier::BOLD);
            put(buf, area, cell, "■", style);
        }

        // 4. Nodes
        for node in &w.nodes {
            let (glyph, style) = node_glyph(node, w.tick);
            put(buf, area, node.pos, glyph, style);
        }

        // 5. Inspector cursor — drawn last so it sits above everything else.
        // We draw a 5-cell crosshair around the cursor: a reverse-video cell
        // at the position plus four bracket marks at the four diagonals so
        // it stays visible regardless of what's underneath.
        if let Some(pos) = self.cursor {
            // Center cell — reverse video on whatever glyph is underneath.
            if pos.0 >= 0 && pos.1 >= 0 {
                let cx = area.x as i32 + pos.0 as i32;
                let cy = area.y as i32 + pos.1 as i32;
                if cx >= area.x as i32
                    && cy >= area.y as i32
                    && cx < area.right() as i32
                    && cy < area.bottom() as i32
                {
                    if let Some(cell) = buf.cell_mut((cx as u16, cy as u16)) {
                        let existing = cell.symbol().to_string();
                        let glyph = if existing.is_empty() || existing == " " {
                            "+".to_string()
                        } else {
                            existing
                        };
                        cell.set_symbol(&glyph).set_style(
                            Style::default()
                                .fg(theme().header_brand_fg)
                                .bg(theme().cursor)
                                .add_modifier(Modifier::BOLD),
                        );
                    }
                }
            }
            // Bracket corners.
            let bracket_style = Style::default()
                .fg(theme().cursor)
                .add_modifier(Modifier::BOLD);
            put(buf, area, (pos.0 - 1, pos.1 - 1), "┌", bracket_style);
            put(buf, area, (pos.0 + 1, pos.1 - 1), "┐", bracket_style);
            put(buf, area, (pos.0 - 1, pos.1 + 1), "└", bracket_style);
            put(buf, area, (pos.0 + 1, pos.1 + 1), "┘", bracket_style);
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

fn infected_glyph(
    node: &Node,
    inf: &crate::world::Infection,
    tick: u64,
) -> (&'static str, Style) {
    let hue = strain_hue(inf.strain);
    match inf.stage {
        InfectionStage::Incubating => {
            // Subtle — same glyph family, but the fg tilts toward strain hue
            // and we drop intensity. Hides the infection until symptoms hit.
            let base = match node.role {
                Role::Relay if node.hardened => "◉",
                Role::Relay => "●",
                Role::Scanner => "◎",
                Role::Exfil => "▣",
                Role::Honeypot => "●",
                // Defenders are immune; this branch shouldn't fire in
                // practice but the match must be exhaustive.
                Role::Defender => "◇",
            };
            (base, Style::default().fg(hue).add_modifier(Modifier::DIM))
        }
        InfectionStage::Active => {
            // Flickers between a block and its normal glyph, strain-colored.
            let base = match node.role {
                Role::Relay if node.hardened => "◉",
                Role::Relay => "●",
                Role::Scanner => "◎",
                Role::Exfil => "▣",
                Role::Honeypot => "●",
                // Defenders are immune; this branch shouldn't fire in
                // practice but the match must be exhaustive.
                Role::Defender => "◇",
            };
            let g = if (tick + inf.age as u64).is_multiple_of(3) {
                "▓"
            } else {
                base
            };
            (
                g,
                Style::default().fg(hue).add_modifier(Modifier::BOLD),
            )
        }
        InfectionStage::Terminal => {
            let st = if tick.is_multiple_of(2) {
                Style::default()
                    .fg(theme().pwned)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default().fg(hue).add_modifier(Modifier::BOLD)
            };
            ("▓", st)
        }
    }
}

fn strain_hue(strain: u8) -> Color {
    let palette = &theme().strain_palette;
    if palette.is_empty() {
        return Color::Magenta;
    }
    palette[(strain as usize) % palette.len()]
}

fn faction_hue(faction: u8) -> Color {
    let palette = &theme().faction_palette;
    if palette.is_empty() {
        return Color::Cyan;
    }
    palette[(faction as usize) % palette.len()]
}

fn branch_hue(branch_id: u16) -> Color {
    let palette = &theme().branch_palette;
    if palette.is_empty() {
        return Color::Green;
    }
    palette[(branch_id as usize) % palette.len()]
}

fn node_glyph(node: &Node, tick: u64) -> (&'static str, Style) {
    let th = theme();
    if node.dying_in > 0 && !matches!(node.state, State::Dead) {
        let st = if (tick + node.dying_in as u64).is_multiple_of(2) {
            Style::default()
                .fg(th.pwned)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default()
                .fg(th.dying_alt)
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
                        .fg(faction_hue(node.faction))
                        .add_modifier(Modifier::BOLD),
                );
            }
            if node.honey_reveal > 0 {
                return (
                    "◈",
                    Style::default()
                        .fg(th.honey_reveal)
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED),
                );
            }
            if node.mutated_flash > 0 {
                let st = if (tick + node.mutated_flash as u64).is_multiple_of(2) {
                    Style::default()
                        .fg(th.mutated_flash_a)
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    Style::default()
                        .fg(th.mutated_flash_b)
                        .add_modifier(Modifier::BOLD)
                };
                return ("✦", st);
            }
            if node.shield_flash > 0 {
                let st = if (tick + node.shield_flash as u64).is_multiple_of(2) {
                    Style::default()
                        .fg(th.shield_flash_a)
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    Style::default()
                        .fg(th.shield_flash_b)
                        .add_modifier(Modifier::BOLD)
                };
                return ("⊕", st);
            }
            if let Some(inf) = node.infection {
                return infected_glyph(node, &inf, tick);
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
                        .fg(th.scanner)
                        .add_modifier(if node.hardened { Modifier::BOLD } else { Modifier::empty() }),
                ),
                Role::Exfil => (
                    "▣",
                    Style::default()
                        .fg(th.exfil)
                        .add_modifier(if node.hardened { Modifier::BOLD } else { Modifier::empty() }),
                ),
                Role::Honeypot => ("●", Style::default().fg(hue)),
                Role::Defender => (
                    "◇",
                    Style::default()
                        .fg(th.defender)
                        .add_modifier(Modifier::BOLD),
                ),
            };
            if pulse_boost {
                (glyph, Style::default().fg(th.value).add_modifier(Modifier::BOLD))
            } else {
                (glyph, base_style)
            }
        }
        State::Pwned { .. } => {
            let st = if tick.is_multiple_of(2) {
                Style::default()
                    .fg(th.pwned)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(th.pwned_alt)
            };
            ("✕", st)
        }
        State::Dead => ("·", Style::default().fg(th.ghost)),
    }
}

