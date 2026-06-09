//! Storage compaction: roll closed sessions older than a cutoff into a single
//! append-only JSONL. Eliminates the 4 KB-per-file block overhead for the
//! historical record.
//!
//! Hot path: per-session JSON files (atomic, simple, in-flight friendly).
//! Cold path: archive.jsonl (cheap, append-only, no block overhead).

use crate::model::SessionRecord;
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};

/// Roll all per-session files older than `cutoff` into `archive.jsonl`,
/// then delete the individual files. Returns the number archived.
///
/// Idempotent: an already-archived file is detected by absence on disk.
/// In-flight sessions (no `ended_at`) are never archived.
pub fn archive_older_than(cutoff: DateTime<Utc>) -> Result<usize> {
    let sessions_dir = crate::paths::sessions_dir()?;
    if !sessions_dir.exists() {
        return Ok(0);
    }
    let archive_path = crate::paths::archive_jsonl()?;
    let mut writer = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&archive_path)
        .with_context(|| format!("opening {}", archive_path.display()))?;

    let mut archived = 0usize;
    for entry in fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let raw = match fs::read_to_string(&path) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let record: SessionRecord = match serde_json::from_str(&raw) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let Some(ended) = record.ended_at else {
            continue; // in-flight: keep
        };
        if ended >= cutoff {
            continue;
        }
        writeln!(writer, "{}", raw.trim())?;
        fs::remove_file(&path).ok();
        archived += 1;
    }
    Ok(archived)
}

/// Load every closed session from `archive.jsonl` whose end time is >= cutoff.
pub fn load_archive_since(cutoff: DateTime<Utc>) -> Result<Vec<SessionRecord>> {
    let path = crate::paths::archive_jsonl()?;
    if !path.exists() {
        return Ok(vec![]);
    }
    let reader = BufReader::new(File::open(&path)?);
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let record: SessionRecord = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let ts = record.ended_at.or(record.started_at);
        if let Some(t) = ts {
            if t >= cutoff {
                out.push(record);
            }
        }
    }
    Ok(out)
}

/// Default archive cutoff: anything ended more than `retention_window_days + 1`
/// days ago. The `+1` grace keeps recently-scored sessions readable as
/// individual files (handy for `claude-time inspect <id>` in v0.2).
pub fn default_archive_cutoff(retention_window_days: u32) -> DateTime<Utc> {
    Utc::now() - Duration::days((retention_window_days as i64) + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SessionRecord;
    use chrono::TimeZone;
    use std::env;
    use tempfile::TempDir;

    fn write_session(dir: &std::path::Path, id: &str, ended_at: DateTime<Utc>) {
        let mut s = SessionRecord::default();
        s.session_id = id.to_string();
        s.cwd = "/whatever".to_string();
        s.started_at = Some(ended_at - Duration::minutes(5));
        s.ended_at = Some(ended_at);
        let path = dir.join(format!("{id}.json"));
        let raw = serde_json::to_string(&s).unwrap();
        std::fs::write(&path, raw).unwrap();
    }

    #[test]
    fn archives_only_old_closed_sessions() {
        let tmp = TempDir::new().unwrap();
        env::set_var("CLAUDE_CONFIG_DIR", tmp.path());
        crate::paths::ensure_dirs().unwrap();

        let sessions = crate::paths::sessions_dir().unwrap();
        let old = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        let recent = Utc::now();
        write_session(&sessions, "old1", old);
        write_session(&sessions, "old2", old);
        write_session(&sessions, "recent1", recent);

        // Cutoff = today - 1d: old1/old2 archived, recent1 stays.
        let cutoff = Utc::now() - Duration::days(1);
        let n = archive_older_than(cutoff).unwrap();
        assert_eq!(n, 2);

        let remaining: Vec<_> = std::fs::read_dir(&sessions)
            .unwrap()
            .filter_map(|e| e.ok().map(|x| x.file_name().into_string().unwrap()))
            .collect();
        assert_eq!(remaining, vec!["recent1.json".to_string()]);

        // Archive contents readable.
        let loaded = load_archive_since(old - Duration::days(1)).unwrap();
        assert_eq!(loaded.len(), 2);

        env::remove_var("CLAUDE_CONFIG_DIR");
    }
}
