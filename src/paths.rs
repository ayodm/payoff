//! Filesystem paths claude-time uses.
//!
//! Honors `CLAUDE_CONFIG_DIR` (same env var Claude Code uses) so a test or
//! sandbox can isolate the data dir from a real `~/.claude/`.

use anyhow::{Context, Result};
use std::path::PathBuf;

/// Root of Claude Code's config dir. `$CLAUDE_CONFIG_DIR` if set, else `~/.claude`.
pub fn claude_config_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let home = dirs::home_dir().context("could not resolve $HOME")?;
    Ok(home.join(".claude"))
}

/// Claude Code's user-level settings file.
pub fn settings_json() -> Result<PathBuf> {
    Ok(claude_config_dir()?.join("settings.json"))
}

/// claude-time's data root. All session records + caches live below this.
pub fn data_dir() -> Result<PathBuf> {
    Ok(claude_config_dir()?.join("claude-time"))
}

pub fn sessions_dir() -> Result<PathBuf> {
    Ok(data_dir()?.join("sessions"))
}

pub fn retention_cache_dir() -> Result<PathBuf> {
    Ok(data_dir()?.join("cache").join("retention"))
}

pub fn config_toml() -> Result<PathBuf> {
    Ok(data_dir()?.join("config.toml"))
}

/// Single append-only JSONL of closed sessions older than the retention window.
/// Eliminates per-file block overhead on the historical record.
pub fn archive_jsonl() -> Result<PathBuf> {
    Ok(data_dir()?.join("archive.jsonl"))
}

/// Create all directories claude-time writes into. Idempotent.
pub fn ensure_dirs() -> Result<()> {
    std::fs::create_dir_all(sessions_dir()?)?;
    std::fs::create_dir_all(retention_cache_dir()?)?;
    Ok(())
}

/// Path to one session's record file.
pub fn session_file(session_id: &str) -> Result<PathBuf> {
    Ok(sessions_dir()?.join(format!("{session_id}.json")))
}
