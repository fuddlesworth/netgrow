//! Tiny formatting helpers shared by main and render.

/// Docker-style memorable name derived from a seed. Same seed always
/// produces the same name, so users can talk about runs ("that
/// elegant-viper-42 run was wild") without memorizing u64s.
pub fn session_name(seed: u64) -> String {
    const ADJ: &[&str] = &[
        "ancient", "brooding", "crimson", "cryptic", "daring", "dormant",
        "eerie", "feral", "ghostly", "hidden", "iron", "jagged",
        "luminous", "mythic", "obsidian", "phantom", "quiet", "rogue",
        "silent", "stoic", "twisted", "umbral", "wandering", "zealous",
    ];
    const NOUN: &[&str] = &[
        "archon", "basilisk", "cipher", "daemon", "echo", "fang",
        "golem", "hydra", "ibis", "jackal", "kraken", "lich",
        "manticore", "nemesis", "oracle", "phoenix", "quarry", "raven",
        "specter", "tyrant", "umbra", "viper", "wraith", "zealot",
    ];
    let adj = ADJ[(seed as usize) % ADJ.len()];
    let noun = NOUN[((seed / ADJ.len() as u64) as usize) % NOUN.len()];
    let num = (seed / (ADJ.len() as u64 * NOUN.len() as u64)) % 100;
    format!("{}-{}-{:02}", adj, noun, num)
}

/// Render a multi-row area graph using Unicode braille characters.
/// Each braille cell is a 2×4 dot grid, so a `width_cells × height_cells`
/// output has `width_cells*2` horizontal resolution and `height_cells*4`
/// vertical resolution — roughly 8x the dot density of a block sparkline
/// of the same area. This is the btop trick.
///
/// Returns `height_cells` strings of `width_cells` chars each, top row
/// first. Filled from the bottom up so it reads like an area chart.
/// Auto-scaled braille area graph — scales each sample against the
/// series' own maximum. Used by the factions panel (via
/// `braille_area_graph_with_max`) and callers that want a single
/// sparkline auto-scaled to itself.
pub fn braille_area_graph_with_max(
    samples: &[u32],
    width_cells: usize,
    height_cells: usize,
    max_value: u32,
) -> Vec<String> {
    let dot_cols = width_cells * 2;
    let dot_rows = height_cells * 4;
    let max = max_value.max(1) as usize;
    let mut fill = vec![0usize; dot_cols];
    if !samples.is_empty() {
        for (i, slot) in fill.iter_mut().enumerate() {
            let sample_idx = (i * samples.len()) / dot_cols;
            let v = samples[sample_idx] as usize;
            *slot = ((v * dot_rows) / max).min(dot_rows);
        }
    }
    render_braille_fill(&fill, width_cells, height_cells)
}

/// Plot a series using min-max normalization within the window,
/// so flat magnitudes still show their internal variation. A
/// perfectly constant series renders as an empty row; any
/// variation amplifies to fill the available cells. Used by the
/// activity panel so a steady mesh doesn't look pinned to the top.
pub fn braille_range_graph(
    samples: &[u32],
    width_cells: usize,
    height_cells: usize,
) -> Vec<String> {
    let dot_cols = width_cells * 2;
    let dot_rows = height_cells * 4;
    let mut fill = vec![0usize; dot_cols];
    if !samples.is_empty() {
        let min = samples.iter().min().copied().unwrap_or(0) as i64;
        let max = samples.iter().max().copied().unwrap_or(0) as i64;
        let range = (max - min).max(1) as usize;
        for (i, slot) in fill.iter_mut().enumerate() {
            let sample_idx = (i * samples.len()) / dot_cols;
            let v = samples[sample_idx] as i64 - min;
            let v = v.max(0) as usize;
            *slot = ((v * dot_rows) / range).min(dot_rows);
        }
    }
    render_braille_fill(&fill, width_cells, height_cells)
}

/// Shared braille-writing core. Takes a per-dot-column fill count
/// and emits the area chart as rows of braille cells.
fn render_braille_fill(
    fill: &[usize],
    width_cells: usize,
    height_cells: usize,
) -> Vec<String> {
    if width_cells == 0 || height_cells == 0 {
        return Vec::new();
    }
    let dot_rows = height_cells * 4;
    // Braille bit layout per cell: [col_offset][row_offset] -> bit mask.
    // Col 0 = left, col 1 = right. Rows 0..=3 = top..=bottom.
    const BITS: [[u8; 4]; 2] = [
        [0x01, 0x02, 0x04, 0x40], // left
        [0x08, 0x10, 0x20, 0x80], // right
    ];
    let mut output = Vec::with_capacity(height_cells);
    for cell_row in 0..height_cells {
        let mut row = String::with_capacity(width_cells);
        for cell_col in 0..width_cells {
            let mut bits: u8 = 0;
            for (col_off, bit_col) in BITS.iter().enumerate() {
                let dot_col = cell_col * 2 + col_off;
                for (row_off, &bit) in bit_col.iter().enumerate() {
                    let abs_row_from_top = cell_row * 4 + row_off;
                    let from_bottom = dot_rows - 1 - abs_row_from_top;
                    if fill.get(dot_col).copied().unwrap_or(0) > from_bottom {
                        bits |= bit;
                    }
                }
            }
            let ch = char::from_u32(0x2800 + bits as u32).unwrap_or(' ');
            row.push(ch);
        }
        output.push(row);
    }
    output
}

/// Single-row braille progress bar. Each cell holds a 2×4 dot grid,
/// so a `width_cells`-cell bar has 2x horizontal sub-cell resolution
/// (half-filled cells look like a partial fill, not a binary on/off).
/// All 4 rows of each dot column light up together, giving a solid
/// bar appearance rather than dots.
pub fn braille_bar(value: u64, max: u64, width_cells: usize) -> String {
    if width_cells == 0 {
        return String::new();
    }
    let total_dots = width_cells * 2;
    let fill = if max == 0 {
        0
    } else {
        ((value as usize * total_dots) / (max as usize)).min(total_dots)
    };
    let mut out = String::with_capacity(width_cells);
    // Left column of each cell = dots 1,2,3,7 → bits 0x01|0x02|0x04|0x40
    // Right column of each cell = dots 4,5,6,8 → bits 0x08|0x10|0x20|0x80
    const LEFT: u8 = 0x01 | 0x02 | 0x04 | 0x40;
    const RIGHT: u8 = 0x08 | 0x10 | 0x20 | 0x80;
    for cell in 0..width_cells {
        let dot_col_left = cell * 2;
        let dot_col_right = dot_col_left + 1;
        let mut bits: u8 = 0;
        if fill > dot_col_left {
            bits |= LEFT;
        }
        if fill > dot_col_right {
            bits |= RIGHT;
        }
        let ch = char::from_u32(0x2800 + bits as u32).unwrap_or(' ');
        out.push(ch);
    }
    out
}

/// Format an integer with thousands separators (commas). Used by the
/// header tick counter so long sessions don't devolve into digit soup.
pub fn with_commas(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}
