use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Layout, Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Widget};
use ratatui::Frame;

use crate::theme::theme;
use crate::util::{braille_area_graph, braille_bar, session_name, sparkline, with_commas};
use crate::world::{
    node_ip, InfectionStage, LinkKind, Node, Role, State, World, WorldStats, HOT_LINK, WARM_LINK,
};

/// Chebyshev radius a territory tint spreads from an alive node into
/// empty background cells. Kept tight so the mesh still breathes;
/// larger values would paint the whole grid at dense populations.
const TERRITORY_RADIUS: i16 = 4;
/// Chars used for the faction territory background tint, indexed by
/// atmospheric mood (day/night/storm). Each is a sparser shade than
/// the previous so the background gets progressively thinner after
/// sundown and during a storm the lines break up entirely.
const TERRITORY_DAY: &str = "░";
const TERRITORY_NIGHT: &str = "·";

const RIGHT_COL_WIDTH: u16 = 41;
const HEADER_HEIGHT: u16 = 1;
const FOOTER_HEIGHT: u16 = 1;

#[derive(Clone, Copy)]
pub struct UiState {
    pub paused: bool,
    pub tick_ms: u64,
    pub seed: u64,
    /// Short name of the theme currently in effect. Used by the footer
    /// indicator; defaults to "cyberpunk" when no theme is loaded.
    pub theme_name: &'static str,
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

    // Mesh border title carries the current era name so the epoch
    // feature surfaces in the chrome. Brand stays in the header.
    let mesh_title = if world.cfg.epoch_period > 0 {
        format!(" {} ", world.epoch_name())
    } else {
        " mesh ".to_string()
    };
    let mesh_block = Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(theme().frame))
        .title(Span::styled(
            mesh_title,
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

    let inspector_height: u16 = if ui.cursor.is_some() { 11 } else { 0 };
    let right_rows = Layout::vertical([
        Constraint::Length(8), // stats: 6 content rows + border
        Constraint::Length(5), // activity: 3 braille content rows + border
        Constraint::Length(8), // roles: 6 content rows + border
        Constraint::Length(inspector_height),
        Constraint::Min(5),
    ])
    .split(right_col);

    frame.render_widget(stats_block(&stats, world.cfg.max_nodes), right_rows[0]);
    frame.render_widget(activity_block(world, right_rows[1].width), right_rows[1]);
    frame.render_widget(legend_block(), right_rows[2]);
    if let Some(pos) = ui.cursor {
        frame.render_widget(inspector_block(world, pos), right_rows[3]);
    }
    frame.render_widget(log_block(world), right_rows[4]);
}

fn activity_block(world: &World, panel_width: u16) -> Paragraph<'static> {
    let th = theme();
    let block = bordered_block(" activity ");
    // Inner width = panel minus two border cells.
    let inner_cells = panel_width.saturating_sub(2) as usize;
    let graph_cells = inner_cells.saturating_sub(0);
    let graph_height = 3usize;
    let samples: Vec<u32> = world.activity_history.iter().copied().collect();
    let rows = braille_area_graph(&samples, graph_cells, graph_height);
    // Gradient: top row dimmer, bottom row normal, matches a subtle
    // "peaks fade into the sky" look.
    let lines: Vec<Line<'static>> = rows
        .into_iter()
        .enumerate()
        .map(|(i, s)| {
            let style = if i == 0 {
                Style::default()
                    .fg(th.frame_accent)
                    .add_modifier(Modifier::DIM)
            } else {
                Style::default()
                    .fg(th.frame_accent)
                    .add_modifier(Modifier::BOLD)
            };
            Line::from(Span::styled(s, style))
        })
        .collect();
    Paragraph::new(lines).block(block)
}

/// ASCII title card for the boot splash. "ANSI Shadow" figlet style —
/// 6 rows tall, ~62 cells wide. Draws with Unicode block characters
/// and box-drawing accents for the shadow highlights.
pub const TITLE_ART: &[&str] = &[
    "███╗   ██╗███████╗████████╗ ██████╗ ██████╗  ██████╗ ██╗    ██╗",
    "████╗  ██║██╔════╝╚══██╔══╝██╔════╝ ██╔══██╗██╔═══██╗██║    ██║",
    "██╔██╗ ██║█████╗     ██║   ██║  ███╗██████╔╝██║   ██║██║ █╗ ██║",
    "██║╚██╗██║██╔══╝     ██║   ██║   ██║██╔══██╗██║   ██║██║███╗██║",
    "██║ ╚████║███████╗   ██║   ╚██████╔╝██║  ██║╚██████╔╝╚███╗███╔╝",
    "╚═╝  ╚═══╝╚══════╝   ╚═╝    ╚═════╝ ╚═╝  ╚═╝ ╚═════╝  ╚══╝╚══╝ ",
];

/// Render the boot splash: the title art centered near the top plus
/// an accumulating list of boot lines below it. Drawn once per step
/// by main.rs during startup, producing a fake "booting" sequence
/// before the real sim takes over.
pub fn draw_boot(frame: &mut Frame, boot_lines: &[String]) {
    let area = frame.area();
    frame.render_widget(Clear, area);
    let th = theme();

    let title_width = TITLE_ART.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u16;
    let boot_width = boot_lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0) as u16;
    let inner_width = title_width.max(boot_width);
    let content_width = (inner_width + 6).min(area.width.saturating_sub(4));
    let content_height = (TITLE_ART.len() as u16 + boot_lines.len() as u16 + 5)
        .min(area.height.saturating_sub(2));
    let box_area = Rect {
        x: area.x + (area.width.saturating_sub(content_width)) / 2,
        y: area.y + (area.height.saturating_sub(content_height)) / 2,
        width: content_width,
        height: content_height,
    };
    let block = Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(th.frame_accent));
    let mut lines: Vec<Line<'static>> = Vec::new();
    for art in TITLE_ART {
        lines.push(Line::from(Span::styled(
            (*art).to_string(),
            Style::default()
                .fg(th.frame_accent)
                .add_modifier(Modifier::BOLD),
        )));
    }
    lines.push(Line::from(""));
    for boot in boot_lines {
        lines.push(Line::from(Span::styled(
            boot.clone(),
            Style::default().fg(th.label),
        )));
    }
    let paragraph = Paragraph::new(lines)
        .block(block)
        .alignment(Alignment::Center);
    frame.render_widget(paragraph, box_area);
}

/// Render a centered session-summary overlay. `lines` is a pre-built
/// list of rows to display — the caller in main.rs decides the exact
/// content so render stays layout-only.
pub fn draw_summary(frame: &mut Frame, lines: &[String]) {
    let area = frame.area();
    // Clear the whole screen first so leftover chrome from the last
    // tick doesn't bleed through.
    frame.render_widget(Clear, area);
    let th = theme();

    let content_width = 56u16.min(area.width.saturating_sub(4));
    let content_height = (lines.len() as u16 + 4).min(area.height.saturating_sub(2));
    let box_area = Rect {
        x: area.x + (area.width.saturating_sub(content_width)) / 2,
        y: area.y + (area.height.saturating_sub(content_height)) / 2,
        width: content_width,
        height: content_height,
    };
    let block = Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(th.frame_accent))
        .title(Span::styled(
            " session summary ",
            Style::default()
                .fg(th.frame_accent)
                .add_modifier(Modifier::BOLD),
        ));
    let text: Vec<Line<'static>> = lines
        .iter()
        .map(|l| {
            // Section headers (indented, uppercase tokens) get an
            // accent color; everything else uses the normal label.
            if l.ends_with(':') {
                Line::from(Span::styled(
                    l.clone(),
                    Style::default()
                        .fg(th.frame_accent)
                        .add_modifier(Modifier::BOLD),
                ))
            } else {
                Line::from(Span::styled(l.clone(), Style::default().fg(th.label)))
            }
        })
        .collect();
    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, box_area);
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
        format!("t={}", with_commas(world.tick)),
        Style::default().fg(th.label),
    ));
    if world.cfg.day_night_period > 0 {
        spans.push(sep());
        if world.is_night() {
            spans.push(Span::styled(
                "☾ night",
                Style::default()
                    .fg(th.stat_packets)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                "☀ day",
                Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
            ));
        }
    }
    if world.is_storming() {
        spans.push(sep());
        let remaining = world.storm_until.saturating_sub(world.tick);
        spans.push(Span::styled(
            format!("⚡ STORM ({})", remaining),
            Style::default()
                .fg(th.pwned)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        ));
    }
    // Era indicator moved to the mesh border title — see MeshWidget.
    spans.push(sep());
    spans.push(stat_span("nodes", format!("{}", stats.alive + stats.pwned)));
    // Prestige readout: always-on, so single-faction runs still show
    // their one C2 and the reader can count how many C2s are alive.
    // Each entry is score + 8-sample trend sparkline, colored by hue.
    if !world.faction_stats.is_empty() {
        spans.push(sep());
        for (i, fs) in world.faction_stats.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" "));
            }
            let hue = faction_hue(i as u8);
            spans.push(Span::styled(
                format!("F{}:{:+}", i, fs.score()),
                Style::default().fg(hue).add_modifier(Modifier::BOLD),
            ));
            let samples: Vec<u32> = fs.history.iter().copied().collect();
            let spark = sparkline(&samples);
            if !spark.is_empty() {
                spans.push(Span::styled(spark, Style::default().fg(hue)));
            }
        }
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
        format!("seed {}", session_name(ui.seed)),
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
        Span::styled("  ·  ", Style::default().fg(th.ghost)),
        Span::styled(
            format!("theme {}", ui.theme_name),
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

fn stats_block(s: &WorldStats, max_nodes: usize) -> Paragraph<'static> {
    let th = theme();
    let block = bordered_block(" stats ");
    let label_style = Style::default().fg(th.stat_label);
    let alive_color = th.branch_palette.first().copied().unwrap_or(th.value);
    let branch_color = th.branch_palette.get(1).copied().unwrap_or(th.value);
    let cap = max_nodes.max(1);
    // Node-population denominator: scales every meter to the shared
    // max_nodes cap so the bars are comparable at a glance.
    let row_with_bar =
        |label: &'static str, value: usize, color: Color| -> Line<'static> {
            let bar = braille_bar(value as u64, cap as u64, 10);
            Line::from(vec![
                Span::styled(format!(" {:<9}", label), label_style),
                Span::styled(
                    format!("{:<4}", value),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(bar, Style::default().fg(color)),
            ])
        };
    let row_plain = |label: &'static str, value: usize, color: Color| -> Line<'static> {
        Line::from(vec![
            Span::styled(format!(" {:<9}", label), label_style),
            Span::styled(
                format!("{}", value),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ])
    };
    let lines = vec![
        row_with_bar("alive", s.alive, alive_color),
        row_with_bar("pwned", s.pwned, th.pwned),
        row_with_bar("dying", s.dying, th.log_cascade),
        row_with_bar("dead", s.dead, th.ghost),
        row_plain("branches", s.branches, branch_color),
        row_plain("bridges", s.cross_links, th.cross_link),
    ];
    Paragraph::new(lines).block(block)
}

fn legend_block() -> Paragraph<'static> {
    let th = theme();
    let block = bordered_block(" roles ");
    let label_style = Style::default().fg(th.label);
    let cell = move |glyph: &'static str,
                     glyph_color: Color,
                     name: &'static str|
          -> Vec<Span<'static>> {
        vec![
            Span::raw(" "),
            Span::styled(
                glyph,
                Style::default().fg(glyph_color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(format!("{:<10}", name), label_style),
        ]
    };
    let row = |a: Vec<Span<'static>>,
               b: Option<Vec<Span<'static>>>,
               c: Option<Vec<Span<'static>>>| {
        let mut spans = a;
        if let Some(b) = b {
            spans.extend(b);
        }
        if let Some(c) = c {
            spans.extend(c);
        }
        Line::from(spans)
    };
    let relay_color = th.branch_palette.first().copied().unwrap_or(th.value);
    // Honeypots are intentionally absent — they masquerade as relays (●)
    // until tripped. Worms and patch waves are also transient-only.
    let lines = vec![
        row(
            cell("◆", faction_hue(0), "c2"),
            Some(cell("●", relay_color, "relay")),
            Some(cell("◉", relay_color, "hardened")),
        ),
        row(
            cell("◎", th.scanner, "scanner"),
            Some(cell("▣", th.exfil, "exfil")),
            Some(cell("◇", th.defender, "defender")),
        ),
        row(
            cell("⊞", th.frame_accent, "tower"),
            Some(cell("⊚", th.accent, "beacon")),
            Some(cell("⊛", th.stat_packets, "proxy")),
        ),
        row(
            cell("⊕", th.value, "router"),
            Some(cell("▓", strain_hue(0), "infected")),
            Some(cell("✕", th.pwned, "pwned")),
        ),
        row(cell("·", th.ghost, "ghost"), None, None),
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
            lines.push(row("ip", node_ip(pos)));
            let role_name = if n.parent.is_none() {
                "C2"
            } else {
                n.role.display_name()
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
                tags.push(format!("{} {}", world.strain_name(inf.strain), stage));
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
        .map(|(s, count)| {
            if *count > 1 {
                color_log_line(&format!("{} (×{})", s, count))
            } else {
                color_log_line(s)
            }
        })
        .collect();
    Paragraph::new(lines).block(block)
}

fn bordered_block(title: &'static str) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Thick)
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
    // Most "node X.Y <suffix>" lines are classified by the portion
    // after the IP. Extract it once up front so matchers can use
    // exact equality / prefix on the suffix instead of contains()
    // substring scans that can false-match.
    let node_suffix: Option<&str> = s
        .strip_prefix("node ")
        .and_then(|rest| rest.split_once(' ').map(|(_, t)| t));

    let style = if s.starts_with("HONEYPOT") {
        Style::default()
            .fg(th.header_brand_fg)
            .bg(th.log_honeypot_bg)
            .add_modifier(Modifier::BOLD)
    } else if s.starts_with("INJECTED") {
        Style::default()
            .fg(th.header_brand_fg)
            .bg(th.log_injected_bg)
            .add_modifier(Modifier::BOLD)
    } else if s.starts_with("ZERO-DAY:") {
        Style::default()
            .fg(th.header_brand_fg)
            .bg(th.log_zero_day_bg)
            .add_modifier(Modifier::BOLD)
    } else if s.starts_with("✦ MYTHIC") {
        Style::default()
            .fg(th.header_brand_fg)
            .bg(th.accent)
            .add_modifier(Modifier::BOLD)
    } else if s.starts_with("⚡ STORM") || s.starts_with("⚡ DDOS") {
        Style::default()
            .fg(th.pwned)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else if s.starts_with("storm passes") {
        Style::default().fg(th.label).add_modifier(Modifier::BOLD)
    } else if s.starts_with("── era") {
        Style::default()
            .fg(th.frame_accent)
            .add_modifier(Modifier::BOLD)
    } else if node_suffix == Some("necrotic") {
        Style::default().fg(th.log_strain).add_modifier(Modifier::BOLD)
    } else if node_suffix == Some("symptomatic") {
        Style::default().fg(th.log_worm).add_modifier(Modifier::BOLD)
    } else if s.contains(" detected at ") {
        Style::default().fg(th.log_strain)
    } else if node_suffix == Some("cured") || node_suffix == Some("patched") {
        Style::default().fg(th.log_cured).add_modifier(Modifier::BOLD)
    } else if s.starts_with("worm delivered") || s.starts_with("worm launched") {
        Style::default().fg(th.log_worm)
    } else if node_suffix
        .map(|t| t.starts_with("mutated "))
        .unwrap_or(false)
    {
        Style::default()
            .fg(th.log_mutated)
            .add_modifier(Modifier::BOLD)
    } else if node_suffix == Some("LOST") || node_suffix == Some("skirmish LOST") {
        Style::default().fg(th.log_lost).add_modifier(Modifier::BOLD)
    } else if s.starts_with("cascade:") || s.starts_with("HONEYPOT cascade:") {
        Style::default()
            .fg(th.log_cascade)
            .add_modifier(Modifier::BOLD)
    } else if node_suffix == Some("hardened") {
        Style::default()
            .fg(th.log_hardened)
            .add_modifier(Modifier::BOLD)
    } else if node_suffix == Some("shielded")
        || node_suffix == Some("skirmish shielded")
        || node_suffix == Some("reinforced")
    {
        Style::default()
            .fg(th.log_shielded)
            .add_modifier(Modifier::BOLD)
    } else if s.starts_with("bridge") {
        Style::default().fg(th.log_bridge)
    } else if s.starts_with("wormhole") {
        Style::default()
            .fg(th.frame_accent)
            .add_modifier(Modifier::BOLD)
    } else if s.starts_with("alliance") {
        Style::default()
            .fg(th.log_cured)
            .add_modifier(Modifier::BOLD)
    } else if s.starts_with("handshake") {
        Style::default().fg(th.log_handshake)
    } else if s.starts_with("beacon") {
        Style::default().fg(th.log_beacon)
    } else if s.starts_with("c2") {
        Style::default()
            .fg(th.log_c2_online)
            .add_modifier(Modifier::BOLD)
    } else if s.starts_with("night falls") {
        Style::default()
            .fg(th.stat_packets)
            .add_modifier(Modifier::BOLD)
    } else if s.starts_with("day breaks") {
        Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
    } else if node_suffix
        .map(|t| t.starts_with("pkt drop"))
        .unwrap_or(false)
    {
        Style::default().fg(th.log_cascade)
    } else if node_suffix
        .map(|t| t.starts_with("backdoor"))
        .unwrap_or(false)
    {
        Style::default()
            .fg(th.log_honeypot_bg)
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
        let th = theme();

        // 0a. Faction territory tint — a multi-source BFS from every
        // alive node stains empty cells within TERRITORY_RADIUS with
        // the nearest faction's hue. Painted first so links, nodes,
        // and every later effect overwrite it naturally. Makes
        // borders, assimilation shifts, and skirmish frontiers
        // visible in the background without competing with foreground
        // mesh state.
        let bounds = w.bounds;
        let mut territory: std::collections::HashMap<(i16, i16), u8> =
            std::collections::HashMap::new();
        let mut queue: std::collections::VecDeque<((i16, i16), u8, i16)> =
            std::collections::VecDeque::new();
        for n in &w.nodes {
            if !matches!(n.state, State::Alive) {
                continue;
            }
            territory.insert(n.pos, n.faction);
            queue.push_back((n.pos, n.faction, 0));
        }
        const NEIGH: [(i16, i16); 8] = [
            (1, 0),
            (-1, 0),
            (0, 1),
            (0, -1),
            (1, 1),
            (1, -1),
            (-1, 1),
            (-1, -1),
        ];
        while let Some((pos, f, d)) = queue.pop_front() {
            if d >= TERRITORY_RADIUS {
                continue;
            }
            for (dx, dy) in NEIGH {
                let np = (pos.0 + dx, pos.1 + dy);
                if np.0 < 0 || np.1 < 0 || np.0 >= bounds.0 || np.1 >= bounds.1 {
                    continue;
                }
                if territory.contains_key(&np) {
                    continue;
                }
                territory.insert(np, f);
                queue.push_back((np, f, d + 1));
            }
        }
        // Atmospheric mood drives which shade glyph the tint uses and
        // whether we skip every other cell. Night thins the tint;
        // storms break it up further. Both are already-tracked sim
        // states we previously only expressed via spawn/loss rate
        // multipliers — now they have a visible footprint too.
        let night = w.is_night();
        let storming = w.is_storming();
        let shade = if night || storming {
            TERRITORY_NIGHT
        } else {
            TERRITORY_DAY
        };
        let node_cells: std::collections::HashSet<(i16, i16)> =
            w.nodes.iter().map(|n| n.pos).collect();
        for (&cell, &fac) in &territory {
            // Don't waste writes under nodes — their glyphs would
            // overwrite us anyway and we want the tint to read as
            // strictly surround rather than underlay.
            if node_cells.contains(&cell) {
                continue;
            }
            // Deterministic stipple based on tick + cell so the
            // pattern gently shimmers without global flicker.
            let key = (cell.0 as u32)
                .wrapping_mul(2654435761)
                ^ (cell.1 as u32).wrapping_mul(40503)
                ^ (w.tick as u32).wrapping_mul(2246822519);
            let thin = night || storming;
            if thin && (key & 1) == 0 {
                continue;
            }
            let style = Style::default()
                .fg(faction_hue(fac))
                .add_modifier(Modifier::DIM);
            put(buf, area, cell, shade, style);
        }

        // 0b. Storm crackle — sparse bright accent flickers scattered
        // across the mesh while a storm is active. Ties directly to
        // the `storm_until` state that otherwise only affects spawn
        // and loss rates, so the viewer can feel the storm in the
        // empty space and not just in the stats panel.
        if storming {
            let density = ((bounds.0 as u32) * (bounds.1 as u32) / 180).max(4);
            for i in 0..density {
                let h = (w.tick as u32)
                    .wrapping_mul(1103515245)
                    .wrapping_add(i.wrapping_mul(2654435761))
                    .wrapping_add(12345);
                let cx = (h % (bounds.0 as u32)) as i16;
                let cy = ((h / (bounds.0 as u32)) % (bounds.1 as u32)) as i16;
                let cell = (cx, cy);
                if node_cells.contains(&cell) {
                    continue;
                }
                put(
                    buf,
                    area,
                    cell,
                    "⁺",
                    Style::default()
                        .fg(th.accent)
                        .add_modifier(Modifier::DIM),
                );
            }
        }

        // 1. Links
        for link in &w.links {
            let a = &w.nodes[link.a];
            let b = &w.nodes[link.b];
            let dying = a.dying_in > 0 || b.dying_in > 0;
            let dead = matches!(a.state, State::Dead) || matches!(b.state, State::Dead);
            let th = theme();
            // A scanner's pulse quietly lifts every wire touching it from
            // its branch hue to the scanner color for SCANNER_PULSE_TICKS
            // ticks — no strobe, no reversed fill, the wire glyphs stay
            // legible, they just brighten.
            let scan_pulse = a.scan_pulse.max(b.scan_pulse);
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
            } else if link.breach_ttl > 0 {
                // Exploit chain trail — dimmed red leading back to C2
                // from the fresh kill. DIM keeps it subordinate to
                // actively-dying links so the eye still reads the live
                // cascade first.
                Style::default()
                    .fg(th.pwned)
                    .add_modifier(Modifier::DIM)
            } else if scan_pulse > 0 {
                Style::default()
                    .fg(th.scanner)
                    .add_modifier(Modifier::BOLD)
            } else if link.kind == LinkKind::Cross {
                Style::default()
                    .fg(th.cross_link)
                    .add_modifier(Modifier::DIM)
            } else if link.load >= HOT_LINK {
                // Saturated — bright cascade color so chokepoints pop.
                Style::default()
                    .fg(th.log_cascade)
                    .add_modifier(Modifier::BOLD)
            } else if link.load >= WARM_LINK {
                // Warming up — accent color, no dim.
                Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
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
                // Loaded links swap from the normal box-drawing glyph
                // to a braille fill character whose dot density scales
                // with the wire's current load. Hot wires read as
                // solid blocks, warm wires as half-fills, idle wires
                // keep their normal path shape.
                let glyph = if link.kind == LinkKind::Parent && !dying && !dead {
                    if link.load >= HOT_LINK {
                        "⣿"
                    } else if link.load >= WARM_LINK + 4 {
                        "⣶"
                    } else if link.load >= WARM_LINK {
                        "⣤"
                    } else {
                        glyph_for(prev, cell, next)
                    }
                } else {
                    glyph_for(prev, cell, next)
                };
                put(buf, area, cell, glyph, style);
            }
        }

        // Patch wave expansion happens silently in the sim layer —
        // advance_patch_waves still propagates cures outward from each
        // C2, but we no longer draw the ○ rings in empty space. The
        // cure itself is visible as the infected node's glyph reverting
        // and a 'cured' line in the log.

        // Scanner pings are rendered by reinterpreting link / node styles
        // in their normal passes — no extra glyphs drawn here.

        // 3. Exfil packets with fading contrails. The head is a bright
        // bold arrow; the next 1-2 cells behind it (toward the child end
        // the packet just came from, i.e. higher path indices) fade out
        // so the viewer sees direction and speed at a glance.
        for pkt in &w.packets {
            let link = &w.links[pkt.link_id];
            let idx = pkt.pos as usize;
            if idx >= link.path.len() {
                continue;
            }
            let a_pos = w.nodes[link.a].pos;
            let b_pos = w.nodes[link.b].pos;
            // Head — bright.
            let head_cell = link.path[idx];
            if head_cell != a_pos && head_cell != b_pos {
                let glyph = packet_glyph(link, idx);
                put(
                    buf,
                    area,
                    head_cell,
                    glyph,
                    Style::default()
                        .fg(theme().packet)
                        .add_modifier(Modifier::BOLD),
                );
            }
            // Tail — two cells behind the head, dimmer with each step.
            for step in 1..=2usize {
                let tail_idx = idx + step;
                if tail_idx >= link.path.len() {
                    break;
                }
                let cell = link.path[tail_idx];
                if cell == a_pos || cell == b_pos {
                    continue;
                }
                let modifier = if step == 1 {
                    Modifier::empty()
                } else {
                    Modifier::DIM
                };
                put(
                    buf,
                    area,
                    cell,
                    "∙",
                    Style::default().fg(theme().packet).add_modifier(modifier),
                );
            }
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

        // 3c. C2 ambient halo — faint braille dots in the 8-cell
        // neighborhood around each C2, colored by faction hue. Creates
        // a subtle "area of influence" marker without cluttering the
        // mesh with full-cell glyphs. Only draws into otherwise-empty
        // cells so it never overrides a node or link glyph.
        let occupied_link_cells: std::collections::HashSet<(i16, i16)> = w
            .links
            .iter()
            .flat_map(|l| l.path.iter().copied())
            .collect();
        for &c2_id in &w.c2_nodes {
            let c2 = &w.nodes[c2_id];
            if !matches!(c2.state, State::Alive) {
                continue;
            }
            let pos = c2.pos;
            let hue = faction_hue(c2.faction);
            let style = Style::default().fg(hue).add_modifier(Modifier::DIM);
            for (dx, dy) in [
                (-1i16, 0i16),
                (1, 0),
                (0, -1),
                (0, 1),
                (-1, -1),
                (1, -1),
                (-1, 1),
                (1, 1),
            ] {
                let cell = (pos.0 + dx, pos.1 + dy);
                if w.occupied.contains(&cell) || occupied_link_cells.contains(&cell) {
                    continue;
                }
                // Pick a braille dot that "points" back toward the C2
                // so the halo reads as light radiating from the center.
                let glyph = match (dx, dy) {
                    (1, 0) => "⡀",
                    (-1, 0) => "⢀",
                    (0, 1) => "⠁",
                    (0, -1) => "⡀",
                    (1, 1) => "⠁",
                    (-1, 1) => "⠁",
                    (1, -1) => "⡀",
                    (-1, -1) => "⡀",
                    _ => "⠂",
                };
                put(buf, area, cell, glyph, style);
            }
        }

        // 4. Nodes
        for node in &w.nodes {
            let (glyph, style) = node_glyph(node, w.tick);
            put(buf, area, node.pos, glyph, style);
        }

        // 4. Wormhole dashed lines — purely visual flash connecting two
        // random alive cells. Rendered as dim braille dots along a
        // Bresenham line so it looks like a rift opening briefly.
        for wh in &w.wormholes {
            // Fade in then fade out: bold at mid-life, dim at edges.
            let life = wh.life.max(1);
            let mid = life / 2;
            let dist_from_mid = (wh.age as i32 - mid as i32).unsigned_abs();
            let intense = dist_from_mid < (mid / 2) as u32;
            let style = if intense {
                Style::default()
                    .fg(theme().frame_accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(theme().frame_accent)
                    .add_modifier(Modifier::DIM)
            };
            // Bresenham line between the two endpoints.
            let (mut x0, mut y0) = (wh.a.0 as i32, wh.a.1 as i32);
            let (x1, y1) = (wh.b.0 as i32, wh.b.1 as i32);
            let dx = (x1 - x0).abs();
            let dy = -(y1 - y0).abs();
            let sx = if x0 < x1 { 1 } else { -1 };
            let sy = if y0 < y1 { 1 } else { -1 };
            let mut err = dx + dy;
            let mut step = 0u32;
            loop {
                // Draw every 3rd cell so the line reads as dashed.
                if step.is_multiple_of(3)
                    && (x0, y0) != (wh.a.0 as i32, wh.a.1 as i32)
                    && (x0, y0) != (x1, y1)
                {
                    put(buf, area, (x0 as i16, y0 as i16), "⠒", style);
                }
                if x0 == x1 && y0 == y1 {
                    break;
                }
                let e2 = 2 * err;
                if e2 >= dy {
                    err += dy;
                    x0 += sx;
                }
                if e2 <= dx {
                    err += dx;
                    y0 += sy;
                }
                step += 1;
            }
        }

        // 4a. DDoS wave front — a line of bold braille blocks across
        // the full row or column where the wave currently sits.
        for wave in &w.ddos_waves {
            let th = theme();
            let style = Style::default()
                .fg(th.pwned)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED);
            if wave.horizontal {
                for x in 0..area.width as i16 {
                    put(buf, area, (x, wave.pos), "⣿", style);
                }
            } else {
                for y in 0..area.height as i16 {
                    put(buf, area, (wave.pos, y), "⣿", style);
                }
            }
        }

        // 4b. Cascade shockwaves and sparks. Shockwaves are a ring of
        // bold braille blocks radiating from the cascade root; sparks
        // are sub-cell dots pooling at their current f32 positions so
        // several sparks in one cell render as distinct dots.
        for sw in &w.shockwaves {
            let age = sw.age as i16;
            if age <= 0 {
                continue;
            }
            let th = theme();
            let dim = sw.age * 2 >= sw.max_age;
            let style = if dim {
                Style::default().fg(th.log_cascade).add_modifier(Modifier::DIM)
            } else {
                Style::default()
                    .fg(th.pwned)
                    .add_modifier(Modifier::BOLD)
            };
            // Thick ring: cells whose Euclidean distance is within
            // 0.5 of the current radius. Uses ⣿ for weight.
            let r = age as f32;
            let r_low = (r - 0.6).max(0.0);
            let r_high = r + 0.6;
            let extent = age + 1;
            for dy in -extent..=extent {
                for dx in -extent..=extent {
                    let d = ((dx * dx + dy * dy) as f32).sqrt();
                    if d >= r_low && d <= r_high {
                        put(buf, area, (sw.origin.0 + dx, sw.origin.1 + dy), "⣿", style);
                    }
                }
            }
        }
        if !w.sparks.is_empty() {
            let th = theme();
            // Group sparks by their integer cell and accumulate
            // braille bits for their sub-cell position.
            let mut groups: std::collections::HashMap<(i16, i16), u8> =
                std::collections::HashMap::new();
            const BITS: [[u8; 4]; 2] = [
                [0x01, 0x02, 0x04, 0x40],
                [0x08, 0x10, 0x20, 0x80],
            ];
            for spark in &w.sparks {
                let cx = spark.pos.0.floor() as i16;
                let cy = spark.pos.1.floor() as i16;
                let fx = (spark.pos.0 - cx as f32).clamp(0.0, 0.9999);
                let fy = (spark.pos.1 - cy as f32).clamp(0.0, 0.9999);
                let dot_col = (fx * 2.0) as usize;
                let dot_row = (fy * 4.0) as usize;
                let bit = BITS[dot_col.min(1)][dot_row.min(3)];
                *groups.entry((cx, cy)).or_insert(0) |= bit;
            }
            for (cell, bits) in groups {
                let ch = char::from_u32(0x2800 + bits as u32).unwrap_or(' ');
                let glyph = ch.to_string();
                // Leak the String to get a &'static — acceptable here
                // because sparks are bounded per frame and we want put()
                // to accept the glyph. But put() takes &str, so we can
                // just pass a reference.
                put(
                    buf,
                    area,
                    cell,
                    glyph.as_str(),
                    Style::default().fg(th.log_cascade).add_modifier(Modifier::BOLD),
                );
            }
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
    // Ransomware override — render as a padlock-ish block in the
    // pwned color regardless of stage.
    if inf.is_ransom {
        let style = if tick.is_multiple_of(4) {
            Style::default()
                .fg(theme().pwned)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default()
                .fg(theme().pwned)
                .add_modifier(Modifier::BOLD)
        };
        return ("⬟", style);
    }
    let hue = strain_hue(inf.strain);
    match inf.stage {
        InfectionStage::Incubating => {
            // Subtle — same glyph family, but the fg tilts toward strain hue
            // and we drop intensity. Hides the infection until symptoms hit.
            let base = if matches!(node.role, Role::Relay) && node.hardened {
                "◉"
            } else {
                node.role.base_glyph()
            };
            (base, Style::default().fg(hue).add_modifier(Modifier::DIM))
        }
        InfectionStage::Active => {
            // Flickers between a block and its normal glyph, strain-colored.
            let base = if matches!(node.role, Role::Relay) && node.hardened {
                "◉"
            } else {
                node.role.base_glyph()
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
                Role::Scanner => {
                    if node.scan_pulse > 0 {
                        // Reversed cell for the pulse duration — a single
                        // localized blink, no strobing, no link flashing.
                        (
                            "◎",
                            Style::default()
                                .fg(th.scanner)
                                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
                        )
                    } else {
                        let m = if node.hardened {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        };
                        ("◎", Style::default().fg(th.scanner).add_modifier(m))
                    }
                }
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
                Role::Tower => (
                    "⊞",
                    Style::default()
                        .fg(th.frame_accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Role::Beacon => (
                    "⊚",
                    Style::default()
                        .fg(th.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Role::Proxy => (
                    "⊛",
                    Style::default()
                        .fg(th.stat_packets)
                        .add_modifier(Modifier::BOLD),
                ),
                // Decoy renders identical to Exfil so it's visually
                // indistinguishable. The inspector reveals the truth.
                Role::Decoy => (
                    "▣",
                    Style::default()
                        .fg(th.exfil)
                        .add_modifier(if node.hardened { Modifier::BOLD } else { Modifier::empty() }),
                ),
                Role::Router => (
                    "⊕",
                    Style::default()
                        .fg(th.value)
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
        State::Dead => {
            // While the ghost echo is still counting down, keep
            // rendering the node's old role glyph dimmed so the kill
            // reads as a fading trace instead of instantly clearing.
            if node.death_echo > 0 {
                (
                    node.role.base_glyph(),
                    Style::default()
                        .fg(th.ghost)
                        .add_modifier(Modifier::DIM),
                )
            } else {
                ("·", Style::default().fg(th.ghost))
            }
        }
    }
}

