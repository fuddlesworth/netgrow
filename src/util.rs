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
pub fn braille_area_graph(
    samples: &[u32],
    width_cells: usize,
    height_cells: usize,
) -> Vec<String> {
    if width_cells == 0 || height_cells == 0 {
        return Vec::new();
    }
    let dot_cols = width_cells * 2;
    let dot_rows = height_cells * 4;
    let empty = char::from_u32(0x2800).unwrap_or(' ').to_string().repeat(width_cells);
    if samples.is_empty() {
        return vec![empty; height_cells];
    }

    let max_sample = samples.iter().max().copied().unwrap_or(1).max(1);
    // fill[dot_col] = number of dot rows filled from the bottom
    let mut fill = vec![0usize; dot_cols];
    for (i, slot) in fill.iter_mut().enumerate() {
        let sample_idx = (i * samples.len()) / dot_cols;
        let v = samples[sample_idx];
        *slot = ((v as usize * dot_rows) / max_sample as usize).min(dot_rows);
    }

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
                    if fill[dot_col] > from_bottom {
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

/// Render a sequence of counts as a compact Unicode block sparkline.
/// Used by the header faction trend indicator — each sample becomes
/// one of ▁▂▃▄▅▆▇█ scaled against the max in the window.
pub fn sparkline(samples: &[u32]) -> String {
    const GLYPHS: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if samples.is_empty() {
        return String::new();
    }
    let max = (*samples.iter().max().unwrap_or(&1)).max(1);
    samples
        .iter()
        .map(|&v| {
            let idx = ((v as usize * (GLYPHS.len() - 1)) / (max as usize).max(1))
                .min(GLYPHS.len() - 1);
            GLYPHS[idx]
        })
        .collect()
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
