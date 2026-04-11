use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ratatui::style::Color;
use serde::Deserialize;

/// Resolved theme used by the renderer. Every color in the UI lives here so
/// that themes can be loaded from disk without touching render.rs.
#[derive(Debug, Clone)]
pub struct Theme {
    // Structural chrome.
    pub frame: Color,
    pub frame_accent: Color,
    pub bar_bg: Color,
    pub label: Color,
    pub value: Color,
    pub ghost: Color,
    pub accent: Color,

    // Header / footer specifics.
    pub header_brand_bg: Color,
    pub header_brand_fg: Color,
    pub stat_label: Color,
    pub stat_value: Color,
    pub stat_packets: Color,
    pub stat_infected: Color,

    // Mesh node states.
    pub pwned: Color,
    pub pwned_alt: Color,
    pub dying_alt: Color,
    pub honey_reveal: Color,
    pub shield_flash_a: Color,
    pub shield_flash_b: Color,
    pub mutated_flash_a: Color,
    pub mutated_flash_b: Color,

    // Role accents (when the role doesn't use a branch hue).
    pub scanner: Color,
    pub exfil: Color,
    pub defender: Color,

    // Travelers / visual effects.
    pub patch_wave: Color,
    pub packet: Color,
    pub cross_link: Color,
    pub ping: Color,
    pub cursor: Color,

    // Palettes.
    pub branch_palette: Vec<Color>,
    pub faction_palette: Vec<Color>,
    pub strain_palette: Vec<Color>,

    // Log line colors keyed by event.
    pub log_handshake: Color,
    pub log_beacon: Color,
    pub log_lost: Color,
    pub log_hardened: Color,
    pub log_shielded: Color,
    pub log_cured: Color,
    pub log_cascade: Color,
    pub log_strain: Color,
    pub log_worm: Color,
    pub log_mutated: Color,
    pub log_bridge: Color,
    pub log_c2_online: Color,
    pub log_default: Color,
    pub log_zero_day_bg: Color,
    pub log_honeypot_bg: Color,
    pub log_injected_bg: Color,
}

impl Default for Theme {
    fn default() -> Self {
        // The cyberpunk default — same colors that were hardcoded before
        // themes existed, frozen here so existing visuals don't drift.
        Self {
            frame: Color::Rgb(60, 180, 200),
            frame_accent: Color::Rgb(120, 220, 240),
            bar_bg: Color::Rgb(10, 20, 30),
            label: Color::Rgb(180, 200, 220),
            value: Color::Rgb(220, 240, 255),
            ghost: Color::Rgb(95, 105, 130),
            accent: Color::Rgb(255, 220, 80),

            header_brand_bg: Color::Rgb(120, 220, 240),
            header_brand_fg: Color::Black,
            stat_label: Color::Rgb(160, 180, 200),
            stat_value: Color::Rgb(200, 220, 240),
            stat_packets: Color::Rgb(120, 240, 255),
            stat_infected: Color::Rgb(220, 120, 240),

            pwned: Color::Red,
            pwned_alt: Color::LightRed,
            dying_alt: Color::LightRed,
            honey_reveal: Color::Yellow,
            shield_flash_a: Color::Rgb(140, 220, 255),
            shield_flash_b: Color::Rgb(200, 240, 255),
            mutated_flash_a: Color::Rgb(255, 120, 220),
            mutated_flash_b: Color::Rgb(255, 180, 240),

            scanner: Color::Rgb(120, 220, 255),
            exfil: Color::Rgb(180, 180, 255),
            defender: Color::Rgb(180, 240, 220),

            patch_wave: Color::Rgb(120, 240, 200),
            packet: Color::Rgb(120, 240, 255),
            cross_link: Color::Rgb(140, 220, 240),
            ping: Color::Rgb(80, 220, 220),
            cursor: Color::Rgb(255, 220, 80),

            branch_palette: vec![
                Color::Rgb(80, 230, 100),
                Color::Rgb(180, 240, 60),
                Color::Rgb(240, 200, 60),
                Color::Rgb(240, 130, 60),
                Color::Rgb(80, 240, 200),
                Color::Rgb(60, 190, 220),
                Color::Rgb(120, 160, 240),
                Color::Rgb(220, 240, 140),
            ],
            faction_palette: vec![
                Color::Rgb(80, 220, 240),  // cyan
                Color::Rgb(255, 160, 60),  // orange
                Color::Rgb(140, 240, 100), // lime green
                Color::Rgb(180, 140, 240), // lavender
                Color::Rgb(255, 100, 160), // hot pink
                Color::Rgb(255, 230, 80),  // yellow
                Color::Rgb(100, 200, 255), // sky blue
                Color::Rgb(240, 130, 90),  // salmon
                Color::Rgb(160, 255, 200), // mint
                Color::Rgb(220, 120, 255), // magenta
                Color::Rgb(255, 200, 120), // peach
                Color::Rgb(100, 250, 170), // jade
            ],
            strain_palette: vec![
                Color::Rgb(220, 80, 220),
                Color::Rgb(180, 100, 240),
                Color::Rgb(230, 120, 200),
                Color::Rgb(160, 60, 200),
                Color::Rgb(240, 140, 230),
                Color::Rgb(200, 100, 170),
                Color::Rgb(190, 80, 220),
                Color::Rgb(240, 100, 240),
            ],

            log_handshake: Color::Rgb(120, 200, 140),
            log_beacon: Color::Rgb(90, 130, 150),
            log_lost: Color::Red,
            log_hardened: Color::Rgb(140, 220, 255),
            log_shielded: Color::Rgb(180, 220, 255),
            log_cured: Color::Rgb(120, 240, 200),
            log_cascade: Color::Rgb(255, 140, 80),
            log_strain: Color::Rgb(200, 100, 200),
            log_worm: Color::Rgb(220, 120, 240),
            log_mutated: Color::Rgb(255, 140, 230),
            log_bridge: Color::Rgb(140, 220, 240),
            log_c2_online: Color::Cyan,
            log_default: Color::Rgb(180, 200, 220),
            log_zero_day_bg: Color::Rgb(255, 220, 80),
            log_honeypot_bg: Color::Yellow,
            log_injected_bg: Color::Rgb(220, 100, 240),
        }
    }
}

/// Process-wide theme. Set once at startup; subsequent reads are cheap and
/// lock-free. Tests and library callers that don't initialize get the
/// default cyberpunk theme automatically.
static THEME: OnceLock<Theme> = OnceLock::new();

pub fn theme() -> &'static Theme {
    THEME.get_or_init(Theme::default)
}

pub fn install(t: Theme) {
    let _ = THEME.set(t);
}

// ---------------------------------------------------------------------------
// File format
// ---------------------------------------------------------------------------

/// Wrapper that deserializes a "#RRGGBB" hex string into a ratatui Color.
#[derive(Debug, Clone, Copy)]
pub struct HexColor(pub Color);

impl<'de> Deserialize<'de> for HexColor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        parse_hex(&s)
            .map(HexColor)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid hex color: {}", s)))
    }
}

fn parse_hex(s: &str) -> Option<Color> {
    let s = s.trim().strip_prefix('#').unwrap_or(s.trim());
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

/// Schema for theme files. Every field is optional so a theme can override
/// just the colors it cares about; the rest fall back to the cyberpunk
/// default.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeFile {
    pub frame: Option<HexColor>,
    pub frame_accent: Option<HexColor>,
    pub bar_bg: Option<HexColor>,
    pub label: Option<HexColor>,
    pub value: Option<HexColor>,
    pub ghost: Option<HexColor>,
    pub accent: Option<HexColor>,

    pub header_brand_bg: Option<HexColor>,
    pub header_brand_fg: Option<HexColor>,
    pub stat_label: Option<HexColor>,
    pub stat_value: Option<HexColor>,
    pub stat_packets: Option<HexColor>,
    pub stat_infected: Option<HexColor>,

    pub pwned: Option<HexColor>,
    pub pwned_alt: Option<HexColor>,
    pub dying_alt: Option<HexColor>,
    pub honey_reveal: Option<HexColor>,
    pub shield_flash_a: Option<HexColor>,
    pub shield_flash_b: Option<HexColor>,
    pub mutated_flash_a: Option<HexColor>,
    pub mutated_flash_b: Option<HexColor>,

    pub scanner: Option<HexColor>,
    pub exfil: Option<HexColor>,
    pub defender: Option<HexColor>,

    pub patch_wave: Option<HexColor>,
    pub packet: Option<HexColor>,
    pub cross_link: Option<HexColor>,
    pub ping: Option<HexColor>,
    pub cursor: Option<HexColor>,

    pub branch_palette: Option<Vec<HexColor>>,
    pub faction_palette: Option<Vec<HexColor>>,
    pub strain_palette: Option<Vec<HexColor>>,

    pub log_handshake: Option<HexColor>,
    pub log_beacon: Option<HexColor>,
    pub log_lost: Option<HexColor>,
    pub log_hardened: Option<HexColor>,
    pub log_shielded: Option<HexColor>,
    pub log_cured: Option<HexColor>,
    pub log_cascade: Option<HexColor>,
    pub log_strain: Option<HexColor>,
    pub log_worm: Option<HexColor>,
    pub log_mutated: Option<HexColor>,
    pub log_bridge: Option<HexColor>,
    pub log_c2_online: Option<HexColor>,
    pub log_default: Option<HexColor>,
    pub log_zero_day_bg: Option<HexColor>,
    pub log_honeypot_bg: Option<HexColor>,
    pub log_injected_bg: Option<HexColor>,
}

impl Theme {
    /// Parse a theme from raw TOML text. `src` is just used in error
    /// messages so the user knows which file/preset blew up.
    pub fn from_toml_str(text: &str, src: &str) -> io::Result<Self> {
        let file: ThemeFile = toml::from_str(text).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("theme parse error in {}: {}", src, e),
            )
        })?;
        let mut t = Theme::default();
        merge(&mut t, file);
        Ok(t)
    }

    /// Load a theme from a filesystem path.
    pub fn load(path: &Path) -> io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Self::from_toml_str(&text, &path.display().to_string())
    }

    /// Resolve a theme reference that may be either a built-in name
    /// (`nord`, `dracula`, …) or a path to a TOML file. Relative paths
    /// resolve against `base_dir` if provided — typically the directory
    /// of the config file that requested the theme.
    pub fn resolve(name_or_path: &str, base_dir: Option<&Path>) -> io::Result<Self> {
        if let Some(text) = builtin_theme(name_or_path) {
            return Self::from_toml_str(text, &format!("builtin:{}", name_or_path));
        }
        let mut p = PathBuf::from(name_or_path);
        if p.is_relative() {
            if let Some(dir) = base_dir {
                p = dir.join(p);
            }
        }
        Self::load(&p)
    }
}

/// Themes shipped with the binary, looked up by short name. Each entry
/// embeds the corresponding TOML file at compile time so users don't
/// need the themes directory on disk.
pub fn builtin_theme(name: &str) -> Option<&'static str> {
    match name {
        "aretha" | "aretha-dark" => Some(include_str!("../themes/aretha-dark.toml")),
        "gruvbox" => Some(include_str!("../themes/gruvbox.toml")),
        "nord" => Some(include_str!("../themes/nord.toml")),
        "dracula" => Some(include_str!("../themes/dracula.toml")),
        "catppuccin" | "catppuccin-mocha" | "mocha" => {
            Some(include_str!("../themes/catppuccin-mocha.toml"))
        }
        "solarized" | "solarized-dark" => Some(include_str!("../themes/solarized-dark.toml")),
        _ => None,
    }
}

/// Sorted list of built-in theme names for `--help` text and listings.
pub const BUILTIN_NAMES: &[&str] = &[
    "aretha-dark",
    "catppuccin-mocha",
    "dracula",
    "gruvbox",
    "nord",
    "solarized-dark",
];

fn merge(t: &mut Theme, f: ThemeFile) {
    macro_rules! single {
        ($($field:ident),* $(,)?) => {
            $( if let Some(c) = f.$field { t.$field = c.0; } )*
        };
    }
    single!(
        frame,
        frame_accent,
        bar_bg,
        label,
        value,
        ghost,
        accent,
        header_brand_bg,
        header_brand_fg,
        stat_label,
        stat_value,
        stat_packets,
        stat_infected,
        pwned,
        pwned_alt,
        dying_alt,
        honey_reveal,
        shield_flash_a,
        shield_flash_b,
        mutated_flash_a,
        mutated_flash_b,
        scanner,
        exfil,
        defender,
        patch_wave,
        packet,
        cross_link,
        ping,
        cursor,
        log_handshake,
        log_beacon,
        log_lost,
        log_hardened,
        log_shielded,
        log_cured,
        log_cascade,
        log_strain,
        log_worm,
        log_mutated,
        log_bridge,
        log_c2_online,
        log_default,
        log_zero_day_bg,
        log_honeypot_bg,
        log_injected_bg,
    );
    if let Some(p) = f.branch_palette {
        if !p.is_empty() {
            t.branch_palette = p.into_iter().map(|c| c.0).collect();
        }
    }
    if let Some(p) = f.faction_palette {
        if !p.is_empty() {
            t.faction_palette = p.into_iter().map(|c| c.0).collect();
        }
    }
    if let Some(p) = f.strain_palette {
        if !p.is_empty() {
            t.strain_palette = p.into_iter().map(|c| c.0).collect();
        }
    }
}
