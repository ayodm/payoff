//! User-overridable configuration at `<data_dir>/config.toml`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub report: ReportConfig,
    #[serde(default)]
    pub exclude: ExcludeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportConfig {
    /// How many days after a session before we score its retention.
    pub retention_window_days: u32,
    /// Operator's effective hourly rate. 0.0 = report only Claude $ cost.
    pub hourly_rate_usd: f64,
    /// Default --since value if the CLI didn't pass one (e.g. "7d", "30d").
    pub default_period: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExcludeConfig {
    /// Path fragments to skip when counting session lines (generated dirs).
    pub paths: Vec<String>,
}

impl Default for ReportConfig {
    fn default() -> Self {
        Self {
            retention_window_days: 7,
            hourly_rate_usd: 0.0,
            default_period: "7d".to_string(),
        }
    }
}

impl Default for ExcludeConfig {
    fn default() -> Self {
        Self {
            paths: vec![
                ".git".to_string(),
                "node_modules".to_string(),
                "_build".to_string(),
                "deps".to_string(),
                "target".to_string(),
                "dist".to_string(),
                ".next".to_string(),
            ],
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            report: ReportConfig::default(),
            exclude: ExcludeConfig::default(),
        }
    }
}

/// Load the config from disk; return defaults if it doesn't exist.
pub fn load() -> Result<Config> {
    let path = crate::paths::config_toml()?;
    if !path.exists() {
        return Ok(Config::default());
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let cfg: Config = toml::from_str(&raw)
        .with_context(|| format!("parsing {}", path.display()))?;
    Ok(cfg)
}
