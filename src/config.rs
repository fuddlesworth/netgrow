use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Persistent configuration loaded from a TOML file. Every field is
/// `Option<T>` so missing keys fall back to the CLI's clap defaults.
/// Explicitly-set CLI flags always override file values.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileConfig {
    pub seed: Option<u64>,
    pub tick_ms: Option<u64>,
    pub spawn_rate: Option<f32>,
    pub loss_rate: Option<f32>,
    pub max_nodes: Option<usize>,

    pub relay_weight: Option<f32>,
    pub scanner_weight: Option<f32>,
    pub exfil_weight: Option<f32>,
    pub honeypot_weight: Option<f32>,
    pub defender_weight: Option<f32>,
    pub tower_weight: Option<f32>,
    pub beacon_weight: Option<f32>,
    pub proxy_weight: Option<f32>,
    pub decoy_weight: Option<f32>,
    pub router_weight: Option<f32>,
    pub hunter_weight: Option<f32>,

    pub isp_outage_period: Option<u64>,
    pub isp_outage_chance: Option<f32>,
    pub isp_outage_life_ticks: Option<u16>,

    pub scanner_ping_period: Option<u16>,
    pub exfil_packet_period: Option<u16>,
    pub hardened_after: Option<u8>,
    pub honeypot_cascade_mult: Option<f32>,

    pub reconnect_rate: Option<f32>,
    pub reconnect_radius: Option<i16>,

    pub virus_spread_rate: Option<f32>,
    pub mutate_rate: Option<f32>,
    pub zero_day_chance: Option<f32>,
    pub ransom_chance: Option<f32>,
    pub carrier_chance: Option<f32>,
    pub cross_faction_bridge_chance: Option<f32>,
    pub assimilation_period: Option<u64>,
    pub disable_virus: Option<bool>,

    pub c2_spawn_bias: Option<f32>,
    pub fork_rate: Option<f32>,
    pub c2_count: Option<u8>,
    pub c2_count_max: Option<u8>,
    pub resurrection_chance: Option<f32>,
    pub day_night_period: Option<u64>,

    /// Optional path to a theme file. Reserved for the next commit;
    /// currently parsed but unused.
    pub theme: Option<String>,
}

impl FileConfig {
    /// Load and parse a TOML config file. Missing files return defaults
    /// (an empty `FileConfig` with every field `None`). Parse errors
    /// propagate so a malformed config doesn't get silently ignored.
    pub fn load(path: &Path) -> io::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str::<FileConfig>(&text).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("config parse error in {}: {}", path.display(), e),
                )
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    /// Conventional default location: `$HOME/.config/netgrow/config.toml`.
    /// Returns `None` if `$HOME` isn't set.
    pub fn default_path() -> Option<PathBuf> {
        std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(".config/netgrow/config.toml"))
    }
}
