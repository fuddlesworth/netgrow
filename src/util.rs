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
