use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Layout, Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Widget};
use ratatui::Frame;

use crate::theme::theme;
use crate::util::{
    braille_area_graph_with_max, braille_range_graph, session_name, with_commas,
};
use crate::world::{
    node_ip, InfectionStage, LinkKind, Node, Role, State, World, WorldStats, HOT_LINK, WARM_LINK,
};

/// Chebyshev radius a territory tint spreads from an alive node into
/// empty background cells. Generous enough that territory blobs
/// merge into continuous regions around clustered factions.
const TERRITORY_RADIUS: i16 = 6;

/// Compute the faction territory BFS: for each alive node, seed
/// its cell with its faction id and expand outward via 8-way
/// Chebyshev steps up to `TERRITORY_RADIUS`. First-arrival wins
/// on contention. Used by both the main mesh render and the
/// minimap so their coverage matches exactly.
fn compute_territory(world: &World) -> std::collections::HashMap<(i16, i16), u8> {
    use std::collections::{HashMap, VecDeque};
    let mut territory: HashMap<(i16, i16), u8> = HashMap::new();
    let mut queue: VecDeque<((i16, i16), u8, i16)> = VecDeque::new();
    for n in &world.meshes[world.active_mesh].nodes {
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
    let (bx, by) = world.meshes[world.active_mesh].bounds;
    while let Some((pos, f, d)) = queue.pop_front() {
        if d >= TERRITORY_RADIUS {
            continue;
        }
        for (dx, dy) in NEIGH {
            let np = (pos.0 + dx, pos.1 + dy);
            if np.0 < 0 || np.1 < 0 || np.0 >= bx || np.1 >= by {
                continue;
            }
            if territory.contains_key(&np) {
                continue;
            }
            territory.insert(np, f);
            queue.push_back((np, f, d + 1));
        }
    }
    territory
}

const RIGHT_COL_WIDTH: u16 = 41;
const HEADER_HEIGHT: u16 = 1;
const FOOTER_HEIGHT: u16 = 1;

/// Which set of panels fills the right column. Runtime is the
/// default game-view with stats/activity/factions/roles; Intel
/// swaps those for info-dense panels (minimap, rivalries, events)
/// that surface state otherwise hidden in the log stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Runtime,
    Intel,
    /// Shadow-map view: brightens dead-node ghosts and
    /// dims every live glyph so the reader can see the mesh's
    /// history — where factions grew, where they died, which
    /// paths used to carry traffic. Good for studying a long
    /// run's shape after the fact.
    Spectral,
}

impl ViewMode {
    pub fn label(&self) -> &'static str {
        match self {
            ViewMode::Runtime => "runtime",
            ViewMode::Intel => "intel",
            ViewMode::Spectral => "spectral",
        }
    }
    pub fn next(&self) -> Self {
        match self {
            ViewMode::Runtime => ViewMode::Intel,
            ViewMode::Intel => ViewMode::Spectral,
            ViewMode::Spectral => ViewMode::Runtime,
        }
    }
}

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
    /// Current right-column view set. Swappable at runtime via the
    /// `v` keybind so the panel real estate can show either the
    /// default runtime readout or the intel view.
    pub view: ViewMode,
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

    // Mesh border title carries the current layer name + era name
    // so the epoch and layer features both surface in the chrome.
    let layer_name = world.meshes[world.active_mesh].name;
    let mesh_title = if world.cfg.epoch_period > 0 {
        format!(
            " {} :: {} ({}/{}) ",
            layer_name,
            world.epoch_name(),
            world.active_mesh + 1,
            world.meshes.len()
        )
    } else {
        format!(" {} ({}/{}) ", layer_name, world.active_mesh + 1, world.meshes.len())
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
            view: ui.view,
        },
        mesh_inner,
    );

    let inspector_height: u16 = if ui.cursor.is_some() { 13 } else { 0 };
    match ui.view {
        ViewMode::Runtime => {
            // factions panel sizes to exactly fit its rows + border. Always
            // reserve at least one content row so the border has something
            // to frame even on a fresh single-faction run.
            let faction_rows = world.faction_stats.len().max(1) as u16;
            let factions_height: u16 = faction_rows + 2;
            let right_rows = Layout::vertical([
                Constraint::Length(7), // stats
                Constraint::Length(5), // activity
                Constraint::Length(factions_height),
                Constraint::Length(7), // roles legend
                Constraint::Length(inspector_height),
                Constraint::Min(5),
            ])
            .split(right_col);
            frame.render_widget(stats_block(world, &stats), right_rows[0]);
            frame.render_widget(activity_block(world, right_rows[1].width), right_rows[1]);
            frame.render_widget(factions_block(world), right_rows[2]);
            frame.render_widget(legend_block(), right_rows[3]);
            if let Some(pos) = ui.cursor {
                frame.render_widget(inspector_block(world, pos), right_rows[4]);
            }
            frame.render_widget(log_block(world, right_rows[5].width), right_rows[5]);
        }
        ViewMode::Intel => {
            // Rivalries / events panels size dynamically to their
            // content (capped so one panel can't eat the column).
            let rivalry_rows: u16 = (world.relations.len().min(6) as u16).max(1);
            let rivalries_height = rivalry_rows + 2;
            let war_count = world
                .relations
                .values()
                .filter(|r| matches!(r.state, crate::world::DiplomaticState::OpenWar))
                .count();
            let event_count = (world.meshes[world.active_mesh].outages.len()
                + world.meshes[world.active_mesh].partitions.len()
                + war_count
                + world.meshes[world.active_mesh].ddos_waves.len()
                + if world.is_storming() { 1 } else { 0 }
                + if world.is_droughted() { 1 } else { 0 })
                .min(6) as u16;
            let events_height = event_count.max(1) + 2;
            let right_rows = Layout::vertical([
                Constraint::Length(10), // minimap
                Constraint::Length(rivalries_height),
                Constraint::Length(events_height),
                Constraint::Length(inspector_height),
                Constraint::Min(5),
            ])
            .split(right_col);
            frame.render_widget(
                minimap_block(world, right_rows[0].width, ui.cursor),
                right_rows[0],
            );
            frame.render_widget(rivalries_block(world), right_rows[1]);
            frame.render_widget(events_block(world), right_rows[2]);
            if let Some(pos) = ui.cursor {
                frame.render_widget(inspector_block(world, pos), right_rows[3]);
            }
            frame.render_widget(log_block(world, right_rows[4].width), right_rows[4]);
        }
        ViewMode::Spectral => {
            // Same sidebar layout as Runtime; the mesh pass
            // itself flips into "shadow" styling via ui.view
            // when drawing nodes and links, so every live glyph
            // dims and every dead ghost brightens.
            let faction_rows = world.faction_stats.len().max(1) as u16;
            let factions_height: u16 = faction_rows + 2;
            let right_rows = Layout::vertical([
                Constraint::Length(7),
                Constraint::Length(5),
                Constraint::Length(factions_height),
                Constraint::Length(7),
                Constraint::Length(inspector_height),
                Constraint::Min(5),
            ])
            .split(right_col);
            frame.render_widget(stats_block(world, &stats), right_rows[0]);
            frame.render_widget(activity_block(world, right_rows[1].width), right_rows[1]);
            frame.render_widget(factions_block(world), right_rows[2]);
            frame.render_widget(legend_block(), right_rows[3]);
            if let Some(pos) = ui.cursor {
                frame.render_widget(inspector_block(world, pos), right_rows[4]);
            }
            frame.render_widget(log_block(world, right_rows[5].width), right_rows[5]);
        }
    }
}

fn activity_block(world: &World, panel_width: u16) -> Paragraph<'static> {
    let th = theme();
    let block = bordered_block(" activity ");
    // Inner width = panel minus two border cells.
    let inner_cells = panel_width.saturating_sub(2) as usize;
    let graph_cells = inner_cells.saturating_sub(0);
    let graph_height = 3usize;
    let samples: Vec<u32> = world.activity_history.iter().copied().collect();
    // Min-max normalization within the window so a steady mesh
    // doesn't pin to the top. The graph now reads as "recent
    // variation" — flat during calm, spiky during churn — which
    // is the more informative signal at a glance.
    let rows = braille_range_graph(&samples, graph_cells, graph_height);
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

/// Context passed to `draw_summary` — everything the end-of-run
/// screen needs to render a full readout. Kept small so main.rs can
/// build it inline from its own locals without reaching into the
/// render layer.
pub struct SummaryMeta<'a> {
    pub session: String,
    pub seed: u64,
    pub theme_name: &'a str,
    pub elapsed: String,
    pub c2_count: u8,
    pub c2_count_max: u8,
    pub spawn_rate: f32,
    pub loss_rate: f32,
    pub virus_spread_rate: Option<f32>,
    pub day_night_period: u64,
}

/// Render the full end-of-run summary: ASCII banner, session meta
/// panel, totals panel, sorted faction leaderboard with medals and
/// score bars, and a footer prompt. Replaces the plain list-of-
/// strings screen with a proper multi-panel layout that makes the
/// exit frame feel intentional.
pub fn draw_summary(frame: &mut Frame, world: &World, meta: &SummaryMeta<'_>) {
    let area = frame.area();
    frame.render_widget(Clear, area);
    let th = theme();

    // Outer frame covers the whole terminal. Everything else is
    // layered inside.
    let outer = Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(th.frame_accent))
        .title(Span::styled(
            " session complete ",
            Style::default()
                .fg(th.frame_accent)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    // Vertical stack: banner / subtitle / meta+totals row / leaderboard / footer.
    let rows = Layout::vertical([
        Constraint::Length(8),  // banner
        Constraint::Length(2),  // subtitle
        Constraint::Length(10), // meta / totals row
        Constraint::Min(6),     // leaderboard
        Constraint::Length(2),  // footer prompt
    ])
    .split(inner);

    // 1. ASCII banner.
    let banner_lines = [
        r" ███╗   ██╗███████╗████████╗ ██████╗ ██████╗  ██████╗ ██╗    ██╗",
        r" ████╗  ██║██╔════╝╚══██╔══╝██╔════╝ ██╔══██╗██╔═══██╗██║    ██║",
        r" ██╔██╗ ██║█████╗     ██║   ██║  ███╗██████╔╝██║   ██║██║ █╗ ██║",
        r" ██║╚██╗██║██╔══╝     ██║   ██║   ██║██╔══██╗██║   ██║██║███╗██║",
        r" ██║ ╚████║███████╗   ██║   ╚██████╔╝██║  ██║╚██████╔╝╚███╔███╔╝",
        r" ╚═╝  ╚═══╝╚══════╝   ╚═╝    ╚═════╝ ╚═╝  ╚═╝ ╚═════╝  ╚══╝╚══╝ ",
    ];
    let banner: Vec<Line<'static>> = banner_lines
        .iter()
        .map(|s| {
            Line::from(Span::styled(
                (*s).to_string(),
                Style::default()
                    .fg(th.frame_accent)
                    .add_modifier(Modifier::BOLD),
            ))
        })
        .collect();
    frame.render_widget(
        Paragraph::new(banner).alignment(Alignment::Center),
        rows[0],
    );

    // 2. Subtitle — ticks + elapsed in one quiet line.
    let alive: usize = world
        .meshes[0]
        .nodes
        .iter()
        .filter(|n| matches!(n.state, State::Alive))
        .count();
    let subtitle_spans = if let Some(winner) = world.current_dominant {
        let hue = faction_hue(world, winner);
        let persona = world
            .personas
            .get(winner as usize)
            .copied()
            .map(|p| p.display_name())
            .unwrap_or("?");
        vec![
            Span::styled(
                "✦ DOMINANCE ✦  ".to_string(),
                Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
            ),
            // Keep 'F0 aggressor' as one faction-hue chunk so the
            // label/number/persona all read as a single unit
            // instead of splitting at 'F' (accent) vs '0' (hue).
            Span::styled(
                format!("F{} {}", winner, persona),
                Style::default().fg(hue).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  ·  "),
            Span::styled(
                meta.elapsed.clone(),
                Style::default().fg(th.value).add_modifier(Modifier::BOLD),
            ),
        ]
    } else {
        vec![
            Span::styled(
                format!("t={}", with_commas(world.tick)),
                Style::default().fg(th.label),
            ),
            Span::raw("  ·  "),
            Span::styled(
                meta.elapsed.clone(),
                Style::default().fg(th.value).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  ·  "),
            Span::styled(
                format!("{} factions", world.faction_stats.len()),
                Style::default().fg(th.label),
            ),
            Span::raw("  ·  "),
            Span::styled(format!("{} alive", alive), Style::default().fg(th.label)),
        ]
    };
    frame.render_widget(
        Paragraph::new(Line::from(subtitle_spans)).alignment(Alignment::Center),
        rows[1],
    );

    // 3. Meta / totals row — split horizontally.
    let mid_cols = Layout::horizontal([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)])
        .split(rows[2]);
    frame.render_widget(summary_meta_block(meta, world), mid_cols[0]);
    frame.render_widget(summary_totals_block(world), mid_cols[1]);

    // 4. Leaderboard + optional legendary roll call beneath.
    let legend_count = world
        .meshes[0]
        .nodes
        .iter()
        .filter(|n| matches!(n.state, State::Alive) && n.legendary_name != u16::MAX)
        .count();
    if legend_count > 0 {
        let inner = Layout::vertical([
            Constraint::Min(3),
            Constraint::Length((legend_count as u16 + 2).min(8)),
        ])
        .split(rows[3]);
        frame.render_widget(summary_leaderboard_block(world), inner[0]);
        frame.render_widget(summary_legends_block(world), inner[1]);
    } else {
        frame.render_widget(summary_leaderboard_block(world), rows[3]);
    }

    // 5. Footer prompt.
    let footer = Line::from(vec![
        Span::styled(
            "▶ press any key to disconnect ",
            Style::default()
                .fg(th.accent)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(footer).alignment(Alignment::Center),
        rows[4],
    );
}

fn summary_meta_block(meta: &SummaryMeta<'_>, world: &World) -> Paragraph<'static> {
    let th = theme();
    let label_style = Style::default().fg(th.stat_label);
    let value_style = Style::default().fg(th.value).add_modifier(Modifier::BOLD);
    let row = |label: &'static str, value: String| {
        Line::from(vec![
            Span::styled(format!(" {:<9}", label), label_style),
            Span::styled(value, value_style),
        ])
    };
    let c2_range = if meta.c2_count_max > meta.c2_count {
        format!("{}..{}", meta.c2_count, meta.c2_count_max)
    } else {
        format!("{}", meta.c2_count)
    };
    let virus = match meta.virus_spread_rate {
        Some(r) => format!("{}", r),
        None => "disabled".to_string(),
    };
    let day_night = if meta.day_night_period == 0 {
        "off".to_string()
    } else {
        format!("{}", meta.day_night_period)
    };
    let lines = vec![
        row("session", meta.session.clone()),
        row("seed", format!("{}", meta.seed)),
        row("era", world.epoch_name().to_string()),
        row("theme", meta.theme_name.to_string()),
        row("c2_count", c2_range),
        row("spawn", format!("{}", meta.spawn_rate)),
        row("loss", format!("{}", meta.loss_rate)),
        row("virus", virus),
        row("day/night", day_night),
    ];
    let block = Block::bordered()
        .border_style(Style::default().fg(th.frame))
        .title(Span::styled(
            " run info ",
            Style::default().fg(th.frame_accent).add_modifier(Modifier::BOLD),
        ));
    Paragraph::new(lines).block(block)
}

fn summary_totals_block(world: &World) -> Paragraph<'static> {
    let th = theme();
    let label_style = Style::default().fg(th.stat_label);
    let value_style = Style::default().fg(th.value).add_modifier(Modifier::BOLD);
    let row = |label: &'static str, value: String, color: Color| {
        Line::from(vec![
            Span::styled(format!(" {:<10}", label), label_style),
            Span::styled(
                value,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ])
    };
    // Global totals aggregated across every faction.
    let total_spawned: u32 = world.faction_stats.iter().map(|f| f.spawned).sum();
    let total_lost: u32 = world.faction_stats.iter().map(|f| f.lost).sum();
    let total_honeys: u32 = world.faction_stats.iter().map(|f| f.honeys_tripped).sum();
    let total_cured: u32 = world.faction_stats.iter().map(|f| f.infections_cured).sum();
    let total_intel: u32 = world.faction_stats.iter().map(|f| f.intel).sum();
    let total_score: i32 = world.faction_stats.iter().map(|f| f.score()).sum();
    let alive: usize = world
        .meshes[0]
        .nodes
        .iter()
        .filter(|n| matches!(n.state, State::Alive))
        .count();
    let dead: usize = world
        .meshes[0]
        .nodes
        .iter()
        .filter(|n| matches!(n.state, State::Dead))
        .count();
    let branches: std::collections::HashSet<u16> = world
        .meshes[0]
        .nodes
        .iter()
        .filter(|n| matches!(n.state, State::Alive))
        .map(|n| n.branch_id)
        .collect();
    let lines = vec![
        Line::from(Span::styled(
            " totals".to_string(),
            Style::default().fg(th.stat_label),
        )),
        row("spawned", format!("{}", total_spawned), th.value),
        row("lost", format!("{}", total_lost), th.pwned),
        row("cured", format!("{}", total_cured), th.defender),
        row("traps", format!("{}", total_honeys), th.accent),
        row("intel", format!("{}", total_intel), th.stat_packets),
        row(
            "shifts",
            format!("{}", world.dominance_shifts),
            th.accent,
        ),
        row(
            "resets",
            format!("{}", world.extinction_cycles),
            th.pwned,
        ),
        row("alive", format!("{}", alive), th.value),
        row("dead", format!("{}", dead), th.ghost),
        row("branches", format!("{}", branches.len()), th.frame_accent),
        Line::from(vec![
            Span::styled(" score     ".to_string(), label_style),
            Span::styled(
                format!("{:+}", total_score),
                value_style,
            ),
        ]),
    ];
    let block = Block::bordered()
        .border_style(Style::default().fg(th.frame))
        .title(Span::styled(
            " totals ",
            Style::default().fg(th.frame_accent).add_modifier(Modifier::BOLD),
        ));
    Paragraph::new(lines).block(block)
}

fn summary_legends_block(world: &World) -> Paragraph<'static> {
    let th = theme();
    let block = Block::bordered()
        .border_style(Style::default().fg(th.frame))
        .title(Span::styled(
            " legends ",
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
        ));
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (id, n) in world.meshes[world.active_mesh].nodes.iter().enumerate() {
        if !matches!(n.state, State::Alive) || n.legendary_name == u16::MAX {
            continue;
        }
        let hue = faction_hue(world, n.faction);
        let bio = legendary_bio(world, n, id);
        lines.push(Line::from(vec![
            Span::styled(" ✦ ", Style::default().fg(th.accent).add_modifier(Modifier::BOLD)),
            Span::styled(
                bio,
                Style::default().fg(hue).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  F{}", n.faction),
                Style::default().fg(th.stat_label),
            ),
        ]));
        if lines.len() >= 6 {
            break;
        }
    }
    Paragraph::new(lines).block(block)
}

fn summary_leaderboard_block(world: &World) -> Paragraph<'static> {
    let th = theme();
    // Rank factions by score descending.
    let mut ordered: Vec<(usize, i32)> = world
        .faction_stats
        .iter()
        .enumerate()
        .map(|(i, fs)| (i, fs.score()))
        .collect();
    ordered.sort_by(|a, b| b.1.cmp(&a.1));
    let max_abs = ordered
        .iter()
        .map(|&(_, s)| s.unsigned_abs())
        .max()
        .unwrap_or(1)
        .max(1);
    let bar_cells = 16u32;
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (rank, &(fid, score)) in ordered.iter().enumerate() {
        let fs = &world.faction_stats[fid];
        let persona = world
            .personas
            .get(fid)
            .copied()
            .map(|p| p.display_name())
            .unwrap_or("?");
        let hue = faction_hue(world, fid as u8);
        let medal = match rank {
            0 => "①",
            1 => "②",
            2 => "③",
            _ => "·",
        };
        let fill = ((score.unsigned_abs() * bar_cells) / max_abs).min(bar_cells);
        let bar: String = (0..bar_cells)
            .map(|i| if i < fill { '█' } else { '░' })
            .collect();
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {} ", medal),
                Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("F{} ", fid),
                Style::default().fg(hue).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{:<11} ", persona),
                Style::default().fg(hue),
            ),
            Span::styled(
                // 7-char width handles up to 6 digits + sign so
                // long-running high-score factions don't overflow
                // the column and push the score bar right.
                format!("{:>+7}  ", score),
                Style::default().fg(hue).add_modifier(Modifier::BOLD),
            ),
            Span::styled(bar, Style::default().fg(hue)),
            Span::styled(
                format!(
                    "  spawn {:<4} lost {:<4} intel {:<5} cured {}",
                    fs.spawned, fs.lost, fs.intel, fs.infections_cured
                ),
                Style::default().fg(th.stat_label),
            ),
        ]));
    }
    let block = Block::bordered()
        .border_style(Style::default().fg(th.frame))
        .title(Span::styled(
            " faction leaderboard ",
            Style::default()
                .fg(th.frame_accent)
                .add_modifier(Modifier::BOLD),
        ));
    Paragraph::new(lines).block(block)
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
        let remaining = world.meshes[world.active_mesh].storm_until.saturating_sub(world.tick);
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
    // Full per-faction prestige readout lives in its own right-column
    // panel (factions_block) so it stops pushing other header stats
    // off the right side on busy multi-faction runs. Keep a compact
    // count here for at-a-glance awareness.
    if !world.faction_stats.is_empty() {
        spans.push(sep());
        spans.push(stat_span(
            "factions",
            format!("{}", world.faction_stats.len()),
        ));
    }
    // Live dominance readout: when a faction holds the majority
    // threshold, show a '✦ F{N} dominant' badge in its own hue
    // so the viewer can see the current leader at a glance.
    if let Some(winner) = world.current_dominant {
        spans.push(sep());
        spans.push(Span::styled(
            format!("✦ F{} dominant", winner),
            Style::default()
                .fg(faction_hue(world, winner))
                .add_modifier(Modifier::BOLD),
        ));
    }
    // Active faction-favoritism boost readout. Shows the boosted
    // faction and the remaining ticks on the window so the viewer
    // knows when their 1-9 nudge will wear off.
    if let Some(fav) = world.favored_faction {
        if world.tick < world.favor_expires_tick {
            let remaining = world.favor_expires_tick.saturating_sub(world.tick);
            spans.push(sep());
            spans.push(Span::styled(
                format!("↑ F{} favored ({}t)", fav, remaining),
                Style::default()
                    .fg(faction_hue(world, fav))
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED),
            ));
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
    // Footer swaps to a cursor-mode help line when the inspector
    // cursor is active, so the available cursor-drop keys are
    // actually discoverable. Default mode stays the same.
    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::raw(" "));
    if ui.cursor.is_some() {
        spans.extend([
            key("⇥"),
            lab(" exit cursor "),
            key("i"),
            lab(" infect "),
            key("p"),
            lab(" patch "),
            key("s"),
            lab(" scan "),
            key("c"),
            lab(" plant c2 "),
            key("w"),
            lab(" wormhole "),
            key("g"),
            lab(" graffiti "),
        ]);
    } else {
        spans.extend([
            key("q"),
            lab(" quit "),
            key("␣"),
            lab(" pause "),
            key("+"),
            key("-"),
            lab(" speed "),
            key("⇥"),
            lab(" cursor "),
            key("v"),
            Span::styled(
                format!(" view ({}) ", ui.view.label()),
                Style::default().fg(th.label),
            ),
            key("["),
            key("]"),
            lab(" layer "),
            key("i"),
            lab(" infect "),
            key("1-9"),
            lab(" favor "),
        ]);
    }
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        format!("{}ms/tick", ui.tick_ms),
        Style::default().fg(th.ghost),
    ));
    spans.push(Span::styled("  ·  ", Style::default().fg(th.ghost)));
    spans.push(Span::styled(
        format!("theme {}", ui.theme_name),
        Style::default().fg(th.ghost),
    ));
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

fn stats_block(world: &World, s: &WorldStats) -> Paragraph<'static> {
    let th = theme();
    let block = bordered_block(" stats ");
    let label_style = Style::default().fg(th.stat_label);
    let alive_color = th.branch_palette.first().copied().unwrap_or(th.value);
    let branch_color = th.branch_palette.get(1).copied().unwrap_or(th.value);
    // Two-column row: one label+value pair on the left, another on
    // the right. Column widths are fixed so every row lines up.
    // Label width 9 (one wider than the longest 8-char label) keeps
    // a guaranteed trailing space before the value.
    let row_pair = |la: &'static str,
                    va: usize,
                    ca: Color,
                    lb: &'static str,
                    vb: usize,
                    cb: Color|
     -> Line<'static> {
        Line::from(vec![
            Span::styled(format!(" {:<9}", la), label_style),
            Span::styled(
                format!("{:>4}", va),
                Style::default().fg(ca).add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled(format!("{:<8}", lb), label_style),
            Span::styled(
                format!("{:>4}", vb),
                Style::default().fg(cb).add_modifier(Modifier::BOLD),
            ),
        ])
    };
    // Derived counters. Routers and legendary nodes scan the node
    // list directly — small enough to stay within a single render
    // frame's budget.
    let routers = world
        .meshes[0]
        .nodes
        .iter()
        .filter(|n| matches!(n.state, State::Alive) && n.role == Role::Router)
        .count();
    let legends = world
        .meshes[0]
        .nodes
        .iter()
        .filter(|n| matches!(n.state, State::Alive) && n.legendary_name != u16::MAX)
        .count();
    // Cumulative pwn count across all factions. Surfaced instead
    // of the transient point-in-time State::Pwned count, which
    // stayed at 0 or 1 almost always and looked broken. Summing
    // faction_stats.lost gives a monotonically-growing lifetime
    // kill counter the viewer can actually watch tick up.
    let cumulative_pwns: usize = world
        .faction_stats
        .iter()
        .map(|fs| fs.lost as usize)
        .sum();
    let lines = vec![
        row_pair(
            "alive",
            s.alive,
            alive_color,
            "pwned",
            cumulative_pwns,
            th.pwned,
        ),
        row_pair("dying", s.dying, th.log_cascade, "dead", s.dead, th.ghost),
        row_pair(
            "branches",
            s.branches,
            branch_color,
            "bridges",
            s.cross_links,
            th.cross_link,
        ),
        row_pair(
            "infected",
            s.infected,
            th.stat_infected,
            "packets",
            s.packets,
            th.stat_packets,
        ),
        row_pair(
            "routers",
            routers,
            th.value,
            "legends",
            legends,
            th.accent,
        ),
    ];
    Paragraph::new(lines).block(block)
}

fn factions_block(world: &World) -> Paragraph<'static> {
    let th = theme();
    let block = bordered_block(" factions ");
    let label_style = Style::default().fg(th.label);
    // Single-row braille area graph — 4x the vertical resolution of
    // the plain unicode-block sparkline, matching the style the
    // activity panel already uses. 14 cells wide leaves room for the
    // F{i}/alive/score prefix in the 41-wide right column.
    const SPARK_CELLS: usize = 14;
    // Shared max across every faction's history so sparklines are
    // spatially comparable — the biggest faction fills the row and
    // smaller ones read as proportionally shorter, instead of every
    // faction auto-scaling to its own max and pinning flat at the
    // top. Give a small headroom so the peak doesn't clip.
    let shared_max = world
        .faction_stats
        .iter()
        .flat_map(|fs| fs.history.iter().copied())
        .max()
        .unwrap_or(1)
        .saturating_add(1)
        .max(1);
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(world.faction_stats.len());
    for (i, fs) in world.faction_stats.iter().enumerate() {
        let hue = faction_hue(world, i as u8);
        let samples: Vec<u32> = fs.history.iter().copied().collect();
        let spark = braille_area_graph_with_max(&samples, SPARK_CELLS, 1, shared_max)
            .into_iter()
            .next()
            .unwrap_or_default();
        let alive = samples.last().copied().unwrap_or(0);
        let persona = world
            .personas
            .get(i)
            .copied()
            .map(|p| p.display_name())
            .unwrap_or("?");
        // Fixed-width 5-char tier badge. All four variants occupy
        // exactly 5 columns so the alive/score/spark cells line up
        // row-to-row — the previous mix (3/4/5 visible chars) put
        // the sparkline in a different column on every row.
        let (tier_glyph, tier_color) = match fs.tech_tier {
            0 => ("·····", theme().ghost),
            1 => ("t1•··", theme().label),
            2 => ("t2••·", theme().accent),
            _ => ("t3•••", theme().pwned),
        };
        // Scores scale with intel-delivering factions (3× intel in
        // `FactionStats::score`), so long runs routinely produce
        // 5-6 digit scores that overflow a `{:>+5}` slot. Collapse
        // anything past 10k into a `+60k` suffix so the column
        // stays exactly 6 chars regardless of magnitude.
        let raw_score = fs.score();
        let score_str = {
            let abs = raw_score.unsigned_abs();
            let sign = if raw_score < 0 { "-" } else { "+" };
            if abs >= 10_000 {
                format!("{}{}k", sign, abs / 1000)
            } else {
                format!("{}{}", sign, abs)
            }
        };
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                format!("F{}", i),
                Style::default().fg(hue).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                // 11 = len("opportunist"), the longest persona name.
                // Anything shorter pushes the alive/score columns
                // out of alignment between rows.
                format!("{:<11}", persona),
                Style::default().fg(hue),
            ),
            Span::raw(" "),
            Span::styled(tier_glyph.to_string(), Style::default().fg(tier_color)),
            Span::raw(" "),
            Span::styled(format!("{:>4}", alive), label_style),
            Span::raw(" "),
            Span::styled(
                format!("{:>6}", score_str),
                Style::default().fg(hue).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(spark, Style::default().fg(hue)),
        ]));
    }
    Paragraph::new(lines).block(block)
}

/// Birds-eye view of the mesh rendered as a coarse grid of cells,
/// each cell colored by the dominant faction of the mesh block it
/// represents. Used by the Intel view's minimap panel.
fn minimap_block(world: &World, panel_width: u16, cursor: Option<(i16, i16)>) -> Paragraph<'static> {
    let th = theme();
    let block = bordered_block(" minimap ");
    let cols = panel_width.saturating_sub(2).max(1) as usize;
    let rows = 8usize;
    let (bx, by) = (world.meshes[world.active_mesh].bounds.0.max(1) as usize, world.meshes[world.active_mesh].bounds.1.max(1) as usize);
    // Minimap mirrors the mesh territory wash: run the same BFS
    // the main render uses, then bucket every tagged cell into
    // the visual grid. Each bucket picks the most common
    // faction, so a cluster of F0 nodes with their surrounding
    // BFS radius paints a solid F0 block in the minimap instead
    // of just the single-cell node positions.
    let territory = compute_territory(world);
    let fac_count = world.faction_stats.len().max(1);
    let mut buckets: Vec<Vec<u32>> = vec![vec![0u32; fac_count]; cols * rows];
    for (&(px, py), &fac) in &territory {
        let mx = (px as usize * cols) / bx.max(1);
        let my = (py as usize * rows) / by.max(1);
        let idx = my * cols + mx;
        if let Some(b) = buckets.get_mut(idx) {
            if let Some(slot) = b.get_mut(fac as usize) {
                *slot += 1;
            }
        }
    }
    // Overlay markers: C2 positions, hotspot cells, and cursor.
    let mut c2_cells: std::collections::HashSet<usize> =
        std::collections::HashSet::new();
    for &c2_id in &world.meshes[world.active_mesh].c2_nodes {
        let n = &world.meshes[world.active_mesh].nodes[c2_id];
        if !matches!(n.state, State::Alive) {
            continue;
        }
        let mx = (n.pos.0 as usize * cols) / bx.max(1);
        let my = (n.pos.1 as usize * rows) / by.max(1);
        c2_cells.insert(my * cols + mx);
    }
    let mut hotspot_cells: std::collections::HashSet<usize> =
        std::collections::HashSet::new();
    for hot in &world.meshes[world.active_mesh].hotspots {
        for y in hot.min.1..=hot.max.1 {
            for x in hot.min.0..=hot.max.0 {
                let mx = (x as usize * cols) / bx.max(1);
                let my = (y as usize * rows) / by.max(1);
                hotspot_cells.insert(my * cols + mx);
            }
        }
    }
    let cursor_idx: Option<usize> = cursor.map(|(cx, cy)| {
        let mx = (cx as usize * cols) / bx.max(1);
        let my = (cy as usize * rows) / by.max(1);
        my * cols + mx
    });
    let lines: Vec<Line<'static>> = (0..rows)
        .map(|y| {
            let spans: Vec<Span<'static>> = (0..cols)
                .map(|x| {
                    let idx = y * cols + x;
                    let b = &buckets[idx];
                    let (best, best_count) = b
                        .iter()
                        .enumerate()
                        .max_by_key(|(_, &c)| c)
                        .map(|(i, &c)| (i, c))
                        .unwrap_or((0, 0));
                    let is_cursor = cursor_idx == Some(idx);
                    let is_c2 = c2_cells.contains(&idx);
                    let is_hotspot = hotspot_cells.contains(&idx);
                    if is_cursor {
                        return Span::styled(
                            "+",
                            Style::default()
                                .fg(th.cursor)
                                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
                        );
                    }
                    if best_count == 0 {
                        if is_hotspot {
                            return Span::styled(
                                "·",
                                Style::default().fg(th.frame_accent),
                            );
                        }
                        return Span::styled(" ", Style::default().fg(th.ghost));
                    }
                    let hue = faction_hue(world, best as u8);
                    if is_c2 {
                        // C2 stands out as a bright bold diamond
                        // on top of its bucket's faction color.
                        return Span::styled(
                            "◆",
                            Style::default().fg(hue).add_modifier(Modifier::BOLD),
                        );
                    }
                    if is_hotspot {
                        // Hotspot overlay on a faction bucket.
                        return Span::styled(
                            "◇",
                            Style::default()
                                .fg(th.frame_accent)
                                .bg(dim_bg(hue))
                                .add_modifier(Modifier::BOLD),
                        );
                    }
                    // Normal territory bucket.
                    Span::styled(" ", Style::default().bg(dim_bg(hue)).fg(hue))
                })
                .collect();
            Line::from(spans)
        })
        .collect();
    Paragraph::new(lines).block(block)
}

/// Top-pressure rivalries rendered as horizontal bars. Pulls from
/// Compact diplomacy panel. For each active relation, draws a
/// state glyph + pair label + the single most informative metric
/// for that state (pressure for hostile pairs, trust for peaceful,
/// overlord pointer for vassalage), plus a dim timer readout if
/// the state is time-bounded. No progress bars — the previous
/// version's low-value fill rendered as dithered noise against
/// the mesh background and wasted horizontal space.
fn rivalries_block(world: &World) -> Paragraph<'static> {
    use crate::world::DiplomaticState;
    let th = theme();
    let block = bordered_block(" diplomacy ");
    if world.relations.is_empty() {
        let lines = vec![Line::from(Span::styled(
            " (all factions Neutral)".to_string(),
            Style::default().fg(th.ghost),
        ))];
        return Paragraph::new(lines).block(block);
    }
    let mut entries: Vec<((u8, u8), crate::world::Relation)> = world
        .relations
        .iter()
        .map(|(&k, &r)| (k, r))
        .collect();
    // Sort by state priority (war first, peace last), then by
    // pressure so the hottest feud lands on top.
    let state_priority = |s: DiplomaticState| -> u8 {
        match s {
            DiplomaticState::OpenWar => 0,
            DiplomaticState::Vassalage { .. } => 1,
            DiplomaticState::ColdWar => 2,
            DiplomaticState::Alliance => 3,
            DiplomaticState::NonAggression => 4,
            DiplomaticState::Trade => 5,
            DiplomaticState::Neutral => 6,
        }
    };
    entries.sort_by(|a, b| {
        state_priority(a.1.state)
            .cmp(&state_priority(b.1.state))
            .then(b.1.pressure.cmp(&a.1.pressure))
    });
    entries.truncate(6);
    let tick = world.tick;
    let lines: Vec<Line<'static>> = entries
        .into_iter()
        .map(|((a, b), rel)| {
            let hue_a = faction_hue(world, a);
            let hue_b = faction_hue(world, b);
            let (mark, mark_color) = match rel.state {
                DiplomaticState::OpenWar => ("⚔", th.pwned),
                DiplomaticState::Vassalage { .. } => ("♛", th.accent),
                DiplomaticState::ColdWar => ("⟡", th.pwned_alt),
                DiplomaticState::Alliance => ("✦", th.accent),
                DiplomaticState::NonAggression => ("◈", th.label),
                DiplomaticState::Trade => ("⇌", th.accent),
                DiplomaticState::Neutral => ("·", th.ghost),
            };
            // Context-sensitive metric: show the number that
            // matters for this state. Pressure for hostilities,
            // trust for cooperation, overlord pointer for
            // vassalage. Neutral pairs only surface a pressure
            // readout when the value is non-zero.
            let (metric, metric_color) = match rel.state {
                DiplomaticState::OpenWar | DiplomaticState::ColdWar => {
                    (format!("p{:>3}", rel.pressure), th.pwned)
                }
                DiplomaticState::Trade
                | DiplomaticState::NonAggression
                | DiplomaticState::Alliance => {
                    (format!("t{:>+3}", rel.trust), th.accent)
                }
                DiplomaticState::Vassalage { overlord } => {
                    (format!("→F{}", overlord), th.accent)
                }
                DiplomaticState::Neutral => {
                    if rel.pressure > 0 {
                        (format!("p{:>3}", rel.pressure), th.stat_label)
                    } else {
                        ("    ".to_string(), th.ghost)
                    }
                }
            };
            // Dim countdown in ticks for timed states. Skipped
            // entirely when expires_tick is 0 (Neutral /
            // Vassalage use death-driven, not timer-driven,
            // exits).
            let timer = if rel.expires_tick > tick {
                format!(" {}t", rel.expires_tick - tick)
            } else {
                String::new()
            };
            Line::from(vec![
                Span::styled(
                    format!(" {} ", mark),
                    Style::default().fg(mark_color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("F{}", a),
                    Style::default().fg(hue_a).add_modifier(Modifier::BOLD),
                ),
                Span::styled("↔".to_string(), Style::default().fg(th.label)),
                Span::styled(
                    format!("F{} ", b),
                    Style::default().fg(hue_b).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:<5}", rel.state.short_label()),
                    Style::default().fg(mark_color),
                ),
                Span::raw(" "),
                Span::styled(metric, Style::default().fg(metric_color)),
                Span::styled(timer, Style::default().fg(th.ghost)),
            ])
        })
        .collect();
    Paragraph::new(lines).block(block)
}

/// Active environmental events panel: storms, DDoS waves, ISP
/// outages, partitions, wars — each with a one-line descriptor
/// and any applicable countdown.
fn events_block(world: &World) -> Paragraph<'static> {
    let th = theme();
    let block = bordered_block(" events ");
    let mut lines: Vec<Line<'static>> = Vec::new();
    let row = |glyph: &'static str, text: String, color: Color| -> Line<'static> {
        Line::from(vec![
            Span::styled(
                format!(" {} ", glyph),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(text, Style::default().fg(th.label)),
        ])
    };
    if world.is_storming() {
        let remaining = world.meshes[world.active_mesh].storm_until.saturating_sub(world.tick);
        lines.push(row("⚡", format!("storm ({}t)", remaining), th.pwned));
    }
    if world.is_droughted() {
        let remaining = world.meshes[world.active_mesh].drought_until.saturating_sub(world.tick);
        lines.push(row("⚠", format!("drought ({}t)", remaining), th.pwned_alt));
    }
    for wave in &world.meshes[world.active_mesh].ddos_waves {
        let axis = if wave.horizontal { "horiz" } else { "vert" };
        lines.push(row("↯", format!("ddos {} pos {}", axis, wave.pos), th.accent));
    }
    for outage in &world.meshes[world.active_mesh].outages {
        let remaining = outage.life.saturating_sub(outage.age);
        lines.push(row(
            "⚠",
            format!("isp outage ({}t)", remaining),
            th.pwned_alt,
        ));
    }
    for part in &world.meshes[world.active_mesh].partitions {
        let remaining = part.life.saturating_sub(part.age);
        let axis = if part.horizontal { "horiz" } else { "vert" };
        lines.push(row(
            "✂",
            format!("partition {} ({}t)", axis, remaining),
            th.pwned_alt,
        ));
    }
    let mut war_pairs: Vec<((u8, u8), u64)> = world
        .relations
        .iter()
        .filter(|(_, r)| matches!(r.state, crate::world::DiplomaticState::OpenWar))
        .map(|(&k, r)| (k, r.expires_tick))
        .collect();
    war_pairs.sort_by(|a, b| b.1.cmp(&a.1));
    for ((a, b), expires) in war_pairs.into_iter().take(4) {
        let remaining = expires.saturating_sub(world.tick);
        lines.push(Line::from(vec![
            Span::styled(
                " ⚔ ".to_string(),
                Style::default().fg(th.pwned).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("F{}", a),
                Style::default().fg(faction_hue(world, a)).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" war ".to_string(), Style::default().fg(th.label)),
            Span::styled(
                format!("F{}", b),
                Style::default().fg(faction_hue(world, b)).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" ({}t)", remaining),
                Style::default().fg(th.stat_label),
            ),
        ]));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            " (all quiet)".to_string(),
            Style::default().fg(th.ghost),
        )));
    }
    lines.truncate(6);
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
            cell("◆", faction_hue_legend(), "c2"),
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
            Some(cell("⟁", th.defender, "hunter")),
            Some(cell("▓", strain_hue(0), "infected")),
        ),
        row(
            cell("◉", th.pwned, "pwned"),
            Some(cell("✕", th.pwned, "dying")),
            Some(cell("·", th.ghost, "ghost")),
        ),
    ];
    Paragraph::new(lines).block(block)
}

/// Generate a one-line biographical flavor string for a legendary
/// node. Deterministic: the template and any randomized details are
/// seeded by the node id + a base pool so the same node always
/// produces the same bio within a run, and across restarts with
/// the same seed.
fn legendary_bio(world: &World, node: &Node, node_id: usize) -> String {
    let pool = crate::world::LEGENDARY_NAME_POOL;
    let name = pool[(node.legendary_name as usize) % pool.len()];
    let age_ticks = world.tick.saturating_sub(node.born);
    let age = if age_ticks >= 1000 {
        format!("{:.1}kt", age_ticks as f32 / 1000.0)
    } else {
        format!("{}t", age_ticks)
    };
    let role = node.role.display_name();
    let children = node.children_spawned;
    // Templates keyed by node id modulo. Each template must fit in
    // the inspector panel's ~30-char value budget to avoid wrapping.
    let templates: [&str; 6] = [
        "{name} — {age} witness",
        "{name} — {role} of b{branch}",
        "{name} — sired {kids}",
        "{name} — veteran {role}",
        "{name} — b{branch} progenitor",
        "{name} — {age} elder, {kids} kin",
    ];
    let idx = (node_id.wrapping_mul(2654435761)) % templates.len();
    let template = templates[idx];
    template
        .replace("{name}", name)
        .replace("{age}", &age)
        .replace("{role}", role)
        .replace("{kids}", &children.to_string())
        .replace("{branch}", &node.branch_id.to_string())
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
    let node = world.meshes[world.active_mesh].nodes.iter().find(|n| n.pos == pos);
    match node {
        None => {
            lines.push(Line::from(Span::styled(
                " (empty cell)".to_string(),
                Style::default().fg(theme().ghost),
            )));
        }
        Some(n) => {
            lines.push(row("ip", node_ip(pos)));
            if n.legendary_name != u16::MAX {
                // Show the generated biographical line instead of
                // just the bare name so legendary nodes read as
                // actual characters.
                let node_id = world.meshes[world.active_mesh].nodes.iter().position(|x| x.pos == pos).unwrap_or(0);
                let bio = legendary_bio(world, n, node_id);
                lines.push(row("legend", bio));
            }
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
            // faction row shows the faction number + persona so the
            // viewer can tell who this node is serving at a glance.
            let persona = world
                .personas
                .get(n.faction as usize)
                .copied()
                .map(|p| p.display_name())
                .unwrap_or("?");
            lines.push(row("faction", format!("{} {}", n.faction, persona)));
            lines.push(row("branch", format!("{}", n.branch_id)));
            let age = world.tick.saturating_sub(n.born);
            lines.push(row("age", format!("{}t", age)));
            lines.push(row("kids", format!("{}", n.children_spawned)));
            // Link count: incoming parent + outgoing children +
            // any cross-links touching this node. Computed from
            // the links vec so the count always reflects live
            // state.
            let link_count = world
                .meshes[0]
                .links
                .iter()
                .filter(|l| {
                    let a_match = world.meshes[world.active_mesh].nodes[l.a].pos == pos;
                    let b_match = world.meshes[world.active_mesh].nodes[l.b].pos == pos;
                    a_match || b_match
                })
                .count();
            if link_count > 0 {
                lines.push(row("links", format!("{}", link_count)));
            }
            if n.pwn_resist > 0 {
                lines.push(row("resist", format!("{}", n.pwn_resist)));
            }
            // Post-cure immunity window still active: show the
            // strain it's immune to and remaining ticks.
            if n.immunity_ticks > 0 {
                if let Some(strain) = n.immunity_strain {
                    lines.push(row(
                        "immune",
                        format!(
                            "{} ({}t)",
                            world.strain_name(strain),
                            n.immunity_ticks
                        ),
                    ));
                }
            }
            // Dedicated infection row so strain + stage + resist +
            // veteran rank all live in one place instead of being
            // squashed into the flags line.
            if let Some(inf) = n.infection {
                let stage = match inf.stage {
                    InfectionStage::Incubating => "inc",
                    InfectionStage::Active => "act",
                    InfectionStage::Terminal => "term",
                };
                let kind = if inf.is_ransom {
                    " RANSOM"
                } else if inf.is_carrier {
                    " CARRIER"
                } else {
                    ""
                };
                let vet = if inf.veteran_rank > 0 {
                    format!(" v{}", inf.veteran_rank)
                } else {
                    String::new()
                };
                lines.push(row(
                    "virus",
                    format!(
                        "{} {} r{}{}{}",
                        world.strain_name(inf.strain),
                        stage,
                        inf.cure_resist,
                        vet,
                        kind
                    ),
                ));
            }
            // Flags: bool and counter badges worth surfacing. Only
            // shows the ones that are currently meaningful so an
            // idle relay's flag row stays short. resist/kids live
            // on their own dedicated rows now.
            let mut tags: Vec<String> = Vec::new();
            if n.hardened {
                tags.push("hardened".into());
            }
            if n.role_cooldown > 0 {
                tags.push(format!("cd {}", n.role_cooldown));
            }
            if !n.hardened && n.heartbeats > 0 {
                tags.push(format!("hb {}/{}", n.heartbeats, world.cfg.hardened_after_heartbeats));
            }
            if n.dying_in > 0 {
                tags.push(format!("dying {}", n.dying_in));
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

fn log_block(world: &World, panel_width: u16) -> Paragraph<'static> {
    let block = bordered_block(" logs ");
    // Inner width = panel width minus the two border cells and one
    // space of left padding. Clamped to a safe floor so we never
    // hand `truncate_to_width` a 0-width budget during startup.
    let inner = (panel_width.saturating_sub(3)).max(10) as usize;
    let lines: Vec<Line<'static>> = world
        .logs
        .iter()
        .rev()
        .take(64)
        .map(|(s, count)| {
            let raw = if *count > 1 {
                format!("{} (×{})", s, count)
            } else {
                s.clone()
            };
            let clipped = truncate_to_width(&raw, inner);
            color_log_line(&clipped)
        })
        .collect();
    Paragraph::new(lines).block(block)
}

/// Truncate `s` so its visible column count fits within `max_cols`,
/// appending `…` when anything gets dropped. Counts chars (not
/// bytes) so multi-byte glyphs like `✦` and `↔` consume one column
/// each — which matches how the terminal renders them in practice
/// for this codebase's log taxonomy. If `max_cols` is smaller than
/// the ellipsis itself, falls back to a hard char-count clamp.
fn truncate_to_width(s: &str, max_cols: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_cols {
        return s.to_string();
    }
    if max_cols <= 1 {
        return chars.into_iter().take(max_cols).collect();
    }
    let keep = max_cols - 1;
    let mut out: String = chars.into_iter().take(keep).collect();
    out.push('…');
    out
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
    } else if s.starts_with("✦ WAR") {
        Style::default()
            .fg(th.header_brand_fg)
            .bg(th.pwned)
            .add_modifier(Modifier::BOLD)
    } else if s.starts_with("✦ DOMINANCE") {
        Style::default()
            .fg(th.header_brand_fg)
            .bg(th.accent)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else if s.starts_with("✦ FALL") {
        Style::default()
            .fg(th.header_brand_fg)
            .bg(th.pwned)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else if s.starts_with("✦ SCORCHED EARTH") {
        Style::default()
            .fg(th.header_brand_fg)
            .bg(th.log_cascade)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else if s.starts_with("✦ sleeper") {
        Style::default()
            .fg(th.header_brand_fg)
            .bg(th.log_injected_bg)
            .add_modifier(Modifier::BOLD)
    } else if s.starts_with("✦ legend") {
        Style::default()
            .fg(th.accent)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else if s.starts_with("✦ favored") {
        Style::default()
            .fg(th.accent)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else if s.starts_with("✦ c2 planted") || s.starts_with("✦ hybrid") {
        Style::default()
            .fg(th.log_mutated)
            .add_modifier(Modifier::BOLD)
    } else if s.starts_with("✦ patent") {
        Style::default()
            .fg(th.stat_packets)
            .add_modifier(Modifier::BOLD)
    } else if s.starts_with("✦ lattice") {
        Style::default()
            .fg(th.cross_link)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else if s.starts_with("✦") {
        // Generic catch-all for any other ✦-prefixed mythic event
        // so nothing in that tier falls through to log_default.
        Style::default()
            .fg(th.accent)
            .add_modifier(Modifier::BOLD)
    } else if s.starts_with("⚡ LINK OVERLOAD") {
        Style::default()
            .fg(th.log_cascade)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else if s.starts_with("⚡ STORM") || s.starts_with("⚡ DDOS") {
        Style::default()
            .fg(th.pwned)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else if s.starts_with("storm passes") {
        Style::default().fg(th.label).add_modifier(Modifier::BOLD)
    } else if s.starts_with("✦ era") {
        Style::default()
            .fg(th.frame_accent)
            .add_modifier(Modifier::BOLD)
    } else if node_suffix == Some("necrotic") {
        Style::default().fg(th.log_strain).add_modifier(Modifier::BOLD)
    } else if node_suffix == Some("symptomatic") {
        Style::default().fg(th.log_worm).add_modifier(Modifier::BOLD)
    } else if s.contains(" CARRIER at ") {
        Style::default()
            .fg(th.header_brand_fg)
            .bg(th.log_strain)
            .add_modifier(Modifier::BOLD)
    } else if s.contains(" ransom at ") {
        Style::default()
            .fg(th.log_strain)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
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
    } else if node_suffix == Some("LOST")
        || node_suffix
            .map(|t| t.starts_with("skirmish LOST"))
            .unwrap_or(false)
    {
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
    } else if s.starts_with("⚠ DROUGHT") || s.starts_with("drought lifts") {
        Style::default().fg(th.pwned_alt).add_modifier(Modifier::BOLD)
    } else if s.starts_with("✦ diplo") {
        Style::default().fg(th.log_cured)
    } else if s.contains("defector") {
        Style::default()
            .fg(th.log_bridge)
            .add_modifier(Modifier::BOLD | Modifier::ITALIC)
    } else if s.starts_with("✦ tech") {
        Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
    } else if s.starts_with("✦ drip") {
        Style::default().fg(th.pwned_alt).add_modifier(Modifier::ITALIC)
    } else if s.starts_with("✦ prophecy") {
        Style::default()
            .fg(th.frame_accent)
            .add_modifier(Modifier::ITALIC)
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
    } else if node_suffix == Some("culled by hunter")
        || node_suffix == Some("antibody cure")
    {
        Style::default().fg(th.defender).add_modifier(Modifier::BOLD)
    } else if node_suffix == Some("antibody launched") {
        Style::default().fg(th.defender)
    } else if node_suffix == Some("backbone link forged") {
        Style::default()
            .fg(th.frame_accent)
            .add_modifier(Modifier::BOLD)
    } else if node_suffix
        .map(|t| t.starts_with("upgraded →"))
        .unwrap_or(false)
    {
        Style::default().fg(th.value).add_modifier(Modifier::BOLD)
    } else if s.starts_with("F") && s.contains("persona shift") {
        Style::default().fg(th.label).add_modifier(Modifier::BOLD)
    } else if s.starts_with("partition healed")
        || s.starts_with("ISP outage cleared")
    {
        Style::default().fg(th.log_cured)
    } else if s.starts_with("⚠ ISP OUTAGE") || s.starts_with("✂ PARTITION") {
        Style::default()
            .fg(th.pwned_alt)
            .add_modifier(Modifier::BOLD)
    } else if s.starts_with("F") && s.contains("loses dominance") {
        Style::default().fg(th.log_cascade).add_modifier(Modifier::BOLD)
    } else if s.starts_with("F") && s.contains("memory fades") {
        Style::default().fg(th.ghost).add_modifier(Modifier::DIM)
    } else if s.starts_with("F") && s.contains("scanner spotted") {
        Style::default().fg(th.scanner)
    } else if s.starts_with("fiber zone") {
        Style::default().fg(th.frame_accent).add_modifier(Modifier::DIM)
    } else if node_suffix == Some("scanner pulse injected")
        || node_suffix == Some("patch wave injected")
        || node_suffix == Some("backdoor revealed")
        || s.starts_with("patch wave injected")
        || s.starts_with("wormhole injected")
        || s.starts_with("scanner pulse refused")
        || s.starts_with("c2 plant refused")
        || s.starts_with("wormhole refused")
        || s.starts_with("inject refused")
    {
        Style::default().fg(th.value).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(th.log_default)
    };
    Line::from(Span::styled(s.to_string(), style))
}

pub struct MeshWidget<'a> {
    pub world: &'a World,
    pub cursor: Option<(i16, i16)>,
    /// Active view mode. Spectral mode tints the whole mesh
    /// toward ghost colors and brightens dead-node echoes so the
    /// reader can study the mesh's history; Runtime and Intel
    /// use the normal styling.
    pub view: ViewMode,
}

impl<'a> Widget for MeshWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let w = self.world;
        let active = w.active_mesh;
        let th = theme();

        // 0a. Faction territory — built via the shared
        // compute_territory helper so the mesh render and the
        // minimap cover the exact same cells. The fg is handled
        // by bg bakes on nodes/links at draw time; the bg
        // post-pass below fills in the empty cells.
        let bounds = w.meshes[active].bounds;
        let territory = compute_territory(w);
        let night = w.is_night();
        let storming = w.is_storming();
        let node_cells: std::collections::HashSet<(i16, i16)> =
            w.meshes[active].nodes.iter().map(|n| n.pos).collect();
        let _ = night; // kept for potential future day/night bg tuning

        // 0a-quater. Fiber hotspot zones — persistent fixed
        // terrain rolled at world creation. Drawn after territory
        // (so the hotspot tint wins over faction shading) but
        // before outages/partitions/nodes (so event overlays
        // still sit on top). Renders as a dim accent-tinted
        // diamond fill + bracket corners so the strategic
        // territory reads as a deliberate box.
        for (idx, hot) in w.meshes[active].hotspots.iter().enumerate() {
            for y in hot.min.1..=hot.max.1 {
                for x in hot.min.0..=hot.max.0 {
                    let cell = (x, y);
                    if node_cells.contains(&cell) {
                        continue;
                    }
                    let key = (x as u32).wrapping_mul(2654435761)
                        ^ (y as u32).wrapping_mul(40503);
                    if !key.is_multiple_of(3) {
                        continue;
                    }
                    put(
                        buf,
                        area,
                        cell,
                        "◈",
                        Style::default().fg(th.frame_accent),
                    );
                }
            }
            let corners = [
                (hot.min, "┏"),
                ((hot.max.0, hot.min.1), "┓"),
                ((hot.min.0, hot.max.1), "┗"),
                (hot.max, "┛"),
            ];
            for (cell, glyph) in corners {
                put(
                    buf,
                    area,
                    cell,
                    glyph,
                    Style::default()
                        .fg(th.accent)
                        .add_modifier(Modifier::BOLD),
                );
            }
            let _ = idx;
        }

        // 0a-bis. ISP outage zones — dim hatched fill across the
        // dead rectangle so the offline region reads at a glance.
        // Drawn after territory so the outage glyph wins on overlap.
        for outage in &w.meshes[active].outages {
            for y in outage.min.1..=outage.max.1 {
                for x in outage.min.0..=outage.max.0 {
                    let cell = (x, y);
                    if node_cells.contains(&cell) {
                        continue;
                    }
                    let key = (x as u32).wrapping_mul(2654435761)
                        ^ (y as u32).wrapping_mul(40503);
                    let glyph = if (key & 0b11) == 0 { "▒" } else { "░" };
                    put(
                        buf,
                        area,
                        cell,
                        glyph,
                        Style::default()
                            .fg(th.pwned_alt)
                            .add_modifier(Modifier::DIM),
                    );
                }
            }
            // Outline corners with brackets so the rectangle reads
            // as a deliberate boundary, not just background mush.
            let corners = [
                (outage.min, "┌"),
                ((outage.max.0, outage.min.1), "┐"),
                ((outage.min.0, outage.max.1), "└"),
                (outage.max, "┘"),
            ];
            for (cell, glyph) in corners {
                put(
                    buf,
                    area,
                    cell,
                    glyph,
                    Style::default()
                        .fg(th.pwned_alt)
                        .add_modifier(Modifier::BOLD),
                );
            }
        }

        // 0a-ter. Network partitions — a dashed horizontal or
        // vertical line cut across the mesh where packets and
        // worms refuse to cross. Draw every other cell so the cut
        // reads as dashed rather than solid.
        for p in &w.meshes[active].partitions {
            if p.horizontal {
                let y = p.pos;
                for x in 0..bounds.0 {
                    if (x as i64 + w.tick as i64).rem_euclid(2) == 0 {
                        continue;
                    }
                    let cell = (x, y);
                    if node_cells.contains(&cell) {
                        continue;
                    }
                    put(
                        buf,
                        area,
                        cell,
                        "─",
                        Style::default()
                            .fg(th.pwned_alt)
                            .add_modifier(Modifier::BOLD | Modifier::DIM),
                    );
                }
            } else {
                let x = p.pos;
                for y in 0..bounds.1 {
                    if (y as i64 + w.tick as i64).rem_euclid(2) == 0 {
                        continue;
                    }
                    let cell = (x, y);
                    if node_cells.contains(&cell) {
                        continue;
                    }
                    put(
                        buf,
                        area,
                        cell,
                        "│",
                        Style::default()
                            .fg(th.pwned_alt)
                            .add_modifier(Modifier::BOLD | Modifier::DIM),
                    );
                }
            }
        }

        // 0b. Storm crackle — a directional front rolling down from
        // the top edge of the mesh. Each active storm picks a
        // direction (dx, 1) at spawn, where dx ∈ {-1, 0, 1} so the
        // front can march straight down, down-left, or down-right.
        // The front wraps: once it exits the bottom it restarts at
        // the top, so the storm keeps visibly sweeping for the full
        // duration instead of rolling once and disappearing.
        if storming {
            let (fdx, fdy) = (w.meshes[active].storm_dir.0 as i16, w.meshes[active].storm_dir.1.max(1) as i16);
            const BAND_HALF: i16 = 3;
            // Cycle length includes the band on both sides of the
            // mesh so the front fully exits before a new one enters,
            // giving a brief 'quiet' gap between passes.
            let cycle = (bounds.1 + BAND_HALF * 2).max(1);
            let raw = w.tick.saturating_sub(w.meshes[active].storm_since) as i64;
            let elapsed = (raw.rem_euclid(cycle as i64) as i16) - BAND_HALF;
            for y in 0..bounds.1 {
                for x in 0..bounds.0 {
                    // Signed distance of this cell from the rolling
                    // front along the (dx, dy) direction. When dx=0
                    // this collapses to `y - elapsed`; with dx!=0
                    // the front tilts diagonally.
                    let dist = (y - elapsed) * fdy + (x * fdx) / 2;
                    if dist.abs() > BAND_HALF {
                        continue;
                    }
                    let cell = (x, y);
                    if node_cells.contains(&cell) {
                        continue;
                    }
                    // Sparse flicker — only ~1 in 6 band cells get a
                    // glyph, with a tick-salted stipple so the
                    // pattern shimmers as the front advances.
                    let key = (x as u32).wrapping_mul(2654435761)
                        ^ (y as u32).wrapping_mul(40503)
                        ^ (w.tick as u32).wrapping_mul(2246822519);
                    if !key.is_multiple_of(6) {
                        continue;
                    }
                    put(
                        buf,
                        area,
                        cell,
                        "⁺",
                        Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
                    );
                }
            }
        }

        // 1. Links
        for link in w.meshes[active].links.iter() {
            if link.latent {
                continue; // sleeper-lattice edges stay invisible
            }
            let a = &w.meshes[active].nodes[link.a];
            let b = &w.meshes[active].nodes[link.b];
            let dying = a.dying_in > 0 || b.dying_in > 0;
            let dead = matches!(a.state, State::Dead) || matches!(b.state, State::Dead);
            let th = theme();
            // A scanner's pulse quietly lifts every wire touching it from
            // its branch hue to the scanner color for SCANNER_PULSE_TICKS
            // ticks — no strobe, no reversed fill, the wire glyphs stay
            // legible, they just brighten.
            let scan_pulse = a.scan_pulse.max(b.scan_pulse);
            // Ghost-link decay: when either endpoint is dead,
            // use the maximum of the two endpoints' death_echo
            // counters to drive a matching fade. Legendary
            // endpoints keep their echo pinned so tombstone
            // links stay visible as permanent remnants.
            let dead_echo: u16 = if dead {
                let ae = if a.legendary_name != u16::MAX {
                    crate::world::GHOST_ECHO_TICKS
                } else {
                    a.death_echo
                };
                let be = if b.legendary_name != u16::MAX {
                    crate::world::GHOST_ECHO_TICKS
                } else {
                    b.death_echo
                };
                ae.max(be)
            } else {
                0
            };
            let dead_legendary = dead
                && (a.legendary_name != u16::MAX || b.legendary_name != u16::MAX);
            // Skip fully-decayed dead links entirely — both
            // endpoints have finished their ghost echo and the
            // cleanup pass has released their cells.
            if dead && !dead_legendary && dead_echo == 0 {
                continue;
            }
            let style = if dying {
                Style::default()
                    .fg(th.pwned)
                    .add_modifier(Modifier::BOLD)
            } else if dead {
                // Non-legendary ghost links fade from a dim
                // ghost-color stroke to nothing. Legendary
                // remnants use a slightly brighter accent so
                // tombstone links stand out from generic ghosts.
                if dead_legendary {
                    Style::default()
                        .fg(th.accent)
                        .add_modifier(Modifier::DIM)
                } else {
                    Style::default()
                        .fg(th.ghost)
                        .add_modifier(Modifier::DIM)
                }
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
                // Links take their color from the child
                // endpoint's faction hue — a faction's whole
                // subtree reads as one color band instead of
                // random branch hues. Main mesh no longer
                // paints a bg wash, so territory identification
                // relies entirely on the shared fg color.
                Style::default().fg(faction_hue(w, b.faction))
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
                if cell == w.meshes[active].nodes[link.a].pos || cell == w.meshes[active].nodes[link.b].pos {
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
                    } else if link.is_backbone {
                        // Idle backbone — thicker box-drawing glyph
                        // hint that this wire has earned its keep.
                        glyph_for_backbone(prev, cell, next)
                    } else {
                        glyph_for(prev, cell, next)
                    }
                } else if dead && !dead_legendary && dead_echo <= crate::world::GHOST_FADE_TICKS {
                    // Deep-decay ghost links shrink from the
                    // box-drawing path into a faint dot trail
                    // before clearing entirely.
                    "·"
                } else {
                    glyph_for(prev, cell, next)
                };
                // Backbones brighten by one notch when idle so the
                // matured chain reads as bolder than a fresh wire,
                // even before any traffic colors it warm/hot.
                let style = if link.is_backbone
                    && !dying
                    && !dead
                    && link.load < WARM_LINK
                {
                    style.add_modifier(Modifier::BOLD)
                } else {
                    style
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
        // Turf graffiti marks: render each active mark as an
        // accent-color `✕` glyph with a pulse that fades as the
        // mark approaches expiry. Drawn before packets so the
        // marker sits under any active traffic glyph, not over it.
        for &(mark_pos, expires) in &w.meshes[active].graffiti_marks {
            let remaining = expires.saturating_sub(w.tick);
            let mod_style = if remaining > crate::world::GRAFFITI_MARK_TICKS / 2 {
                Modifier::BOLD
            } else {
                Modifier::empty()
            };
            put(
                buf,
                area,
                mark_pos,
                "✕",
                Style::default().fg(theme().accent).add_modifier(mod_style),
            );
        }
        for pkt in &w.meshes[active].packets {
            let link = &w.meshes[active].links[pkt.link_id];
            let idx = pkt.pos as usize;
            if idx >= link.path.len() {
                continue;
            }
            let a_pos = w.meshes[active].nodes[link.a].pos;
            let b_pos = w.meshes[active].nodes[link.b].pos;
            // Head — bright. Ghost packets use the ghost palette
            // color and drop the BOLD modifier so the decoy
            // stream reads as translucent against the real
            // traffic.
            let head_cell = link.path[idx];
            if head_cell != a_pos && head_cell != b_pos {
                let glyph = packet_glyph(link, idx);
                let (head_color, head_mod) = if pkt.ghost {
                    (theme().ghost, Modifier::empty())
                } else {
                    (theme().packet, Modifier::BOLD)
                };
                put(
                    buf,
                    area,
                    head_cell,
                    glyph,
                    Style::default().fg(head_color).add_modifier(head_mod),
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
        for worm in &w.meshes[active].worms {
            let link = &w.meshes[active].links[worm.link_id];
            let idx = worm.pos as usize;
            if idx >= link.path.len() {
                continue;
            }
            let cell = link.path[idx];
            if cell == w.meshes[active].nodes[link.a].pos || cell == w.meshes[active].nodes[link.b].pos {
                continue;
            }
            // Antibody worms render as a distinct green diamond so
            // the viewer can tell counter-attacks apart from
            // infection worms at a glance.
            let (glyph, style) = if worm.is_antibody {
                (
                    "◈",
                    Style::default()
                        .fg(th.defender)
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED),
                )
            } else {
                (
                    "■",
                    Style::default()
                        .fg(strain_hue(worm.strain))
                        .add_modifier(Modifier::BOLD),
                )
            };
            put(buf, area, cell, glyph, style);
        }

        // 3c. C2 ambient halo — faint braille dots in the 8-cell
        // neighborhood around each C2, colored by faction hue. Creates
        // a subtle "area of influence" marker without cluttering the
        // mesh with full-cell glyphs. Only draws into otherwise-empty
        // cells so it never overrides a node or link glyph.
        let occupied_link_cells: std::collections::HashSet<(i16, i16)> = w
            .meshes[0]
            .links
            .iter()
            .flat_map(|l| l.path.iter().copied())
            .collect();
        for &c2_id in &w.meshes[active].c2_nodes {
            let c2 = &w.meshes[active].nodes[c2_id];
            if !matches!(c2.state, State::Alive) {
                continue;
            }
            let pos = c2.pos;
            let hue = faction_hue(w, c2.faction);
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
                if w.meshes[active].occupied.contains(&cell) || occupied_link_cells.contains(&cell) {
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
        for node in &w.meshes[active].nodes {
            let (glyph, style) = node_glyph(node, w.tick, w);
            put(buf, area, node.pos, glyph, style);
        }

        // 4. Wormhole dashed lines — purely visual flash connecting two
        // random alive cells. Rendered as dim braille dots along a
        // Bresenham line so it looks like a rift opening briefly.
        for wh in &w.meshes[active].wormholes {
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
        for wave in &w.meshes[active].ddos_waves {
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
        for sw in &w.meshes[active].shockwaves {
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
        if !w.meshes[active].sparks.is_empty() {
            let th = theme();
            // Group sparks by their integer cell and accumulate
            // braille bits for their sub-cell position.
            let mut groups: std::collections::HashMap<(i16, i16), u8> =
                std::collections::HashMap::new();
            const BITS: [[u8; 4]; 2] = [
                [0x01, 0x02, 0x04, 0x40],
                [0x08, 0x10, 0x20, 0x80],
            ];
            for spark in &w.meshes[active].sparks {
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

        // 4b. Ambient background post-pass.
        //
        // The main mesh no longer paints a territory bg wash —
        // factions are identified by the fg color of their nodes
        // and links instead, so every cell stays fg-only.
        //
        // Hotspots and outages still get bg tints because they
        // mark fixed strategic terrain the viewer needs to see
        // regardless of who currently owns the region.
        let hotspot_color = hotspot_bg(th.frame_accent);
        for hot in &w.meshes[active].hotspots {
            for y in hot.min.1..=hot.max.1 {
                for x in hot.min.0..=hot.max.0 {
                    let cx = area.x as i32 + x as i32;
                    let cy = area.y as i32 + y as i32;
                    if cx < area.x as i32
                        || cy < area.y as i32
                        || cx >= area.right() as i32
                        || cy >= area.bottom() as i32
                    {
                        continue;
                    }
                    if let Some(c) = buf.cell_mut((cx as u16, cy as u16)) {
                        if c.bg == Color::Reset {
                            c.set_bg(hotspot_color);
                        }
                    }
                }
            }
        }
        let outage_color = outage_bg(th.pwned_alt);
        for outage in &w.meshes[active].outages {
            for y in outage.min.1..=outage.max.1 {
                for x in outage.min.0..=outage.max.0 {
                    let cx = area.x as i32 + x as i32;
                    let cy = area.y as i32 + y as i32;
                    if cx < area.x as i32
                        || cy < area.y as i32
                        || cx >= area.right() as i32
                        || cy >= area.bottom() as i32
                    {
                        continue;
                    }
                    if let Some(c) = buf.cell_mut((cx as u16, cy as u16)) {
                        c.set_bg(outage_color);
                    }
                }
            }
        }
        let _ = &territory; // map still used by minimap via compute_territory

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

        // Spectral view: walk every cell in the rendered area
        // and fold its fg toward the ghost color, stripping BOLD
        // so live glyphs dim and ghost tombstones stand out.
        // Dead-node glyphs already render in their ghost hue so
        // this pass lets them read as the "bright layer" while
        // everything else recedes.
        if matches!(self.view, ViewMode::Spectral) {
            let ghost = theme().ghost;
            for y in area.y..area.bottom() {
                for x in area.x..area.right() {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        let sym = cell.symbol().to_string();
                        if sym == " " {
                            continue;
                        }
                        // Skip cells already in the ghost color —
                        // those are the dead tombstones / decayed
                        // links we want to preserve at full
                        // brightness.
                        let style = cell.style();
                        if style.fg == Some(ghost) {
                            continue;
                        }
                        let dimmed = Style::default()
                            .fg(ghost)
                            .add_modifier(Modifier::DIM);
                        cell.set_style(dimmed);
                    }
                }
            }
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

/// Heavy-stroke variant of `glyph_for`, used for idle backbone links so
/// matured wires read as visibly thicker than fresh ones.
fn glyph_for_backbone(
    prev: Option<(i16, i16)>,
    cur: (i16, i16),
    next: Option<(i16, i16)>,
) -> &'static str {
    let dir = |a: (i16, i16), b: (i16, i16)| (b.0 - a.0, b.1 - a.1);
    match (prev, next) {
        (Some(p), Some(n)) => {
            let d1 = dir(p, cur);
            let d2 = dir(cur, n);
            if d1.0 == 0 && d2.0 == 0 {
                "┃"
            } else if d1.1 == 0 && d2.1 == 0 {
                "━"
            } else {
                match (d1, d2) {
                    ((1, 0), (0, 1)) | ((0, -1), (-1, 0)) => "┓",
                    ((1, 0), (0, -1)) | ((0, 1), (-1, 0)) => "┛",
                    ((-1, 0), (0, 1)) | ((0, -1), (1, 0)) => "┏",
                    ((-1, 0), (0, -1)) | ((0, 1), (1, 0)) => "┗",
                    _ => "·",
                }
            }
        }
        (None, Some(n)) | (Some(n), None) => {
            let d = dir(cur, n);
            if d.0 == 0 {
                "┃"
            } else if d.1 == 0 {
                "━"
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

/// Darken a hue to a background-safe shade: dark enough that the
/// foreground glyph stays readable, but saturated enough that
/// each faction's territory is unmistakably its own color.
/// Used for every per-cell bg — nodes, links, C2s, and the
/// ambient post-pass for empty cells — so the whole region
/// renders as a single continuous colored wash.
fn dim_bg(c: Color) -> Color {
    match c {
        Color::Rgb(r, g, b) => Color::Rgb(
            (r as f32 * 0.50) as u8,
            (g as f32 * 0.50) as u8,
            (b as f32 * 0.50) as u8,
        ),
        other => other,
    }
}

/// Slightly brighter background shade used for fiber-hotspot
/// cells — ~25% of the accent hue so the zone reads through the
/// territory tint but still doesn't compete with foreground.
fn hotspot_bg(c: Color) -> Color {
    match c {
        Color::Rgb(r, g, b) => Color::Rgb(
            (r as f32 * 0.28) as u8,
            (g as f32 * 0.28) as u8,
            (b as f32 * 0.28) as u8,
        ),
        other => other,
    }
}

/// Darker red tint used for ISP outage cells so the dead region
/// reads as a bruise regardless of whose territory it overlaps.
fn outage_bg(c: Color) -> Color {
    match c {
        Color::Rgb(r, g, b) => Color::Rgb(
            (r as f32 * 0.30) as u8,
            (g as f32 * 0.12) as u8,
            (b as f32 * 0.12) as u8,
        ),
        other => other,
    }
}


fn faction_hue(world: &World, faction: u8) -> Color {
    let palette = &theme().faction_palette;
    if palette.is_empty() {
        return Color::Cyan;
    }
    // Unaffiliated mercenaries render in a neutral ghost color
    // so they visibly stand apart from any faction's hue.
    if faction == crate::world::MERCENARY_FACTION {
        return theme().ghost;
    }
    let idx = world.faction_color_index(faction) % palette.len();
    palette[idx]
}

/// Legend-only variant that doesn't need a World — picks palette[0]
/// directly so the legend glyph always has a fixed demo hue.
fn faction_hue_legend() -> Color {
    theme()
        .faction_palette
        .first()
        .copied()
        .unwrap_or(Color::Cyan)
}

fn node_glyph(node: &Node, tick: u64, world: &World) -> (&'static str, Style) {
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
                        .fg(faction_hue(world, node.faction))
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
            let hue = faction_hue(world, node.faction);
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
                Role::Hunter => (
                    "⟁",
                    Style::default()
                        .fg(th.defender)
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED),
                ),
            };
            let mut resolved = if pulse_boost {
                (glyph, Style::default().fg(th.value).add_modifier(Modifier::BOLD))
            } else {
                (glyph, base_style)
            };
            // Legendary nodes keep a permanent underlined highlight
            // on top of whatever role styling resolved — subtle
            // enough to not clobber existing reads, distinct enough
            // to pick out the named characters at a glance.
            if node.legendary_name != u16::MAX {
                resolved.1 = resolved
                    .1
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
            }
            resolved
        }
        State::Pwned { .. } => {
            // Distinct 'freshly compromised' look — reversed block
            // flashing between two red tones, using a different
            // glyph (◉) than the dying cascade's ✕ so the viewer
            // can tell 'just exploited' apart from 'cascading out'.
            let st = if tick.is_multiple_of(2) {
                Style::default()
                    .fg(th.pwned)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default()
                    .fg(th.pwned_alt)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            };
            ("◉", st)
        }
        State::Dead => {
            // Legendary dead nodes are permanent tombstones —
            // they never decay away and keep a bold '†' glyph on
            // the mesh so the viewer can find their bio in the
            // inspector after the fall.
            if node.legendary_name != u16::MAX {
                return (
                    "†",
                    Style::default()
                        .fg(th.accent)
                        .add_modifier(Modifier::BOLD),
                );
            }
            // Non-legendary ghost decay stages, driven by the
            // death_echo countdown:
            //
            //   > GHOST_FADE_TICKS  → old role glyph, dim ghost
            //   1..=GHOST_FADE_TICKS → faint `·` ghost
            //   0                    → nothing (empty cell); the
            //                          cleanup pass has already
            //                          released the cell from
            //                          `occupied` so new traffic
            //                          can reclaim it.
            if node.death_echo > crate::world::GHOST_FADE_TICKS {
                (
                    node.role.base_glyph(),
                    Style::default()
                        .fg(th.ghost)
                        .add_modifier(Modifier::DIM),
                )
            } else if node.death_echo > 0 {
                ("·", Style::default().fg(th.ghost).add_modifier(Modifier::DIM))
            } else {
                (" ", Style::default())
            }
        }
    }
}

