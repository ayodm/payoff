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

/// payoff's data root. All session records + caches live below this.
///
/// On first call, if `~/.claude/claude-time/` exists from a prior
/// claude-time install and `~/.claude/payoff/` does not, rename the legacy
/// dir over. This is a best-effort migration — failure logs to stderr but
/// doesn't propagate, so a permission glitch doesn't break the hook.
pub fn data_dir() -> Result<PathBuf> {
    let new_dir = claude_config_dir()?.join("payoff");
    if !new_dir.exists() {
        let legacy = claude_config_dir()?.join("claude-time");
        if legacy.exists() {
            if let Err(err) = std::fs::rename(&legacy, &new_dir) {
                eprintln!(
                    "[payoff] could not migrate legacy data dir {} -> {}: {err}",
                    legacy.display(),
                    new_dir.display()
                );
            }
        }
    }
    Ok(new_dir)
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
///
/// Rejects session IDs that would be unsafe as filenames — anything outside
/// `[A-Za-z0-9_-]`, or longer than 64 chars. Real Claude Code session IDs
/// are UUIDv4 (36 chars, hex+dashes) and pass cleanly; this is purely
/// defensive against hand-crafted or malformed payloads that could otherwise
/// write outside the sessions dir (e.g. `../escape`) or trigger OS errors
/// (e.g. `a/b`).
pub fn session_file(session_id: &str) -> Result<PathBuf> {
    validate_session_id(session_id)?;
    Ok(sessions_dir()?.join(format!("{session_id}.json")))
}

fn validate_session_id(session_id: &str) -> Result<()> {
    if session_id.is_empty() {
        anyhow::bail!("session_id is empty");
    }
    if session_id.len() > 64 {
        anyhow::bail!("session_id too long ({} chars, max 64)", session_id.len());
    }
    if !session_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!("session_id contains unsafe characters (only [A-Za-z0-9_-] allowed)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_v4_passes() {
        // Real Claude Code session IDs.
        assert!(validate_session_id("b48618ce-190f-4777-98fb-f6986d0c1712").is_ok());
        assert!(validate_session_id("ABCD-1234").is_ok());
        assert!(validate_session_id("under_score").is_ok());
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(validate_session_id("../escape").is_err());
        assert!(validate_session_id("..").is_err());
        assert!(validate_session_id("a/b").is_err());
        assert!(validate_session_id("a\\b").is_err());
    }

    #[test]
    fn rejects_special_chars() {
        for bad in [
            " ",
            "with space",
            "with;semi",
            "with\"quote",
            "with$dollar",
            "🦀",
        ] {
            assert!(
                validate_session_id(bad).is_err(),
                "expected reject: {bad:?}"
            );
        }
    }

    #[test]
    fn rejects_too_long() {
        let long = "x".repeat(65);
        assert!(validate_session_id(&long).is_err());
        let max = "x".repeat(64);
        assert!(validate_session_id(&max).is_ok());
    }

    #[test]
    fn rejects_empty() {
        assert!(validate_session_id("").is_err());
    }

    #[test]
    fn rejects_control_chars() {
        assert!(validate_session_id("a\nb").is_err());
        assert!(validate_session_id("a\0b").is_err());
        assert!(validate_session_id("a\tb").is_err());
    }
}
