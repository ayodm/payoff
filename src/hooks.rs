//! Entry points called by Claude Code's `SessionStart` and `SessionEnd` hooks.
//!
//! Claude Code invokes us with the hook payload on stdin (JSON). We capture
//! whatever passive signal is available at that moment — git HEAD, transcript
//! contents, etc. — and merge it into a `SessionRecord` keyed by `session_id`.
//!
//! Hooks must never crash a Claude Code session. Any I/O or parse error is
//! swallowed with a stderr warning; we'd rather lose one session's data than
//! interrupt the user's work.

use crate::cli::HookEvent;
use crate::model::SessionRecord;
use anyhow::{Context, Result};
use chrono::Utc;
use git2::Repository;
use serde_json::Value;
use std::io::Read;
use std::path::Path;

pub fn run(event: HookEvent) -> Result<()> {
    // Wrap the whole flow in a soft error: a hook never propagates failure.
    if let Err(err) = run_inner(event) {
        eprintln!("[claude-time] hook {:?} failed: {err:#}", event);
    }
    Ok(())
}

fn run_inner(event: HookEvent) -> Result<()> {
    crate::paths::ensure_dirs()?;

    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw)?;
    let payload: Value = if raw.trim().is_empty() {
        Value::Object(Default::default())
    } else {
        serde_json::from_str(&raw).context("parsing hook payload")?
    };

    let session_id = payload
        .get("session_id")
        .and_then(|v| v.as_str())
        .context("hook payload missing session_id")?
        .to_string();

    let session_path = crate::paths::session_file(&session_id)?;
    let mut record = load_or_default(&session_path, &session_id);
    populate_from_payload(&mut record, &payload);

    match event {
        HookEvent::SessionStart => populate_session_start(&mut record),
        HookEvent::SessionEnd => populate_session_end(&mut record, &payload),
    }

    // Compact JSON, not pretty — ~50% smaller on disk and humans rarely read
    // these files directly anyway. `claude-time inspect <session>` (v0.2)
    // pretty-prints on demand.
    let serialized = serde_json::to_string(&record)?;
    std::fs::write(&session_path, serialized)
        .with_context(|| format!("writing {}", session_path.display()))?;
    Ok(())
}

fn load_or_default(path: &Path, session_id: &str) -> SessionRecord {
    if !path.exists() {
        let mut s = SessionRecord::default();
        s.session_id = session_id.to_string();
        return s;
    }
    match std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<SessionRecord>(&raw).ok())
    {
        Some(r) => r,
        None => {
            let mut s = SessionRecord::default();
            s.session_id = session_id.to_string();
            s
        }
    }
}

fn populate_from_payload(record: &mut SessionRecord, payload: &Value) {
    if record.cwd.is_empty() {
        if let Some(cwd) = payload.get("cwd").and_then(|v| v.as_str()) {
            record.cwd = cwd.to_string();
            // Derive a friendly project name from the last path segment.
            record.project = Path::new(cwd)
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string());
        }
    }
    if record.transcript_path.is_none() {
        if let Some(p) = payload.get("transcript_path").and_then(|v| v.as_str()) {
            record.transcript_path = Some(p.to_string());
        }
    }
    if record.model.is_none() {
        if let Some(m) = payload.get("model").and_then(|v| v.as_str()) {
            record.model = Some(m.to_string());
        }
    }
    // Capture unknown fields so future Claude Code payload keys land
    // somewhere without requiring a schema migration to surface them.
    let extras = crate::env_capture::payload_extras(payload);
    if !extras.is_empty() {
        record.env_extras = extras;
    }
}

fn populate_session_start(record: &mut SessionRecord) {
    if record.started_at.is_none() {
        record.started_at = Some(Utc::now());
    }
    if let Some((sha, branch)) = read_git_head(&record.cwd) {
        if record.git_sha_before.is_none() {
            record.git_sha_before = Some(sha);
        }
        if record.branch.is_none() {
            record.branch = Some(branch);
        }
    }
    // Snapshot the prompt environment (skills, CLAUDE.md, hooks, plugins).
    // Wrapped: any panic in env_capture is swallowed, never blocks SessionStart.
    if !record.cwd.is_empty() {
        let cwd = record.cwd.clone();
        let env = std::panic::catch_unwind(|| crate::env_capture::capture(Path::new(&cwd)));
        match env {
            Ok(env) => crate::env_capture::apply(record, env),
            Err(_) => eprintln!("[claude-time] env_capture panicked; skipping driver fields"),
        }
    }
}

fn populate_session_end(record: &mut SessionRecord, _payload: &Value) {
    record.ended_at = Some(Utc::now());

    if let Some((sha, _)) = read_git_head(&record.cwd) {
        record.git_sha_after = Some(sha);
    }

    // Parse transcript for tool + token stats (best-effort).
    if let Some(path) = record.transcript_path.clone() {
        match crate::transcript::parse(Path::new(&path)) {
            Ok(stats) => {
                record.turn_count = stats.turn_count;
                record.tool_calls = stats.tool_calls;
                record.per_file_edits = stats.per_file_edits;
                record.tool_sequence = stats.tool_sequence;
                if record.files_modified.is_empty() {
                    record.files_modified = stats.files_modified;
                }
                record.total_cost_usd = stats.total_cost_usd;
                record.input_tokens = stats.input_tokens;
                record.output_tokens = stats.output_tokens;
                record.cache_read_tokens = stats.cache_read_tokens;
                record.cache_creation_tokens = stats.cache_creation_tokens;
            }
            Err(err) => eprintln!("[claude-time] transcript parse failed: {err:#}"),
        }
    }

    // Per-file diff between sha_before and sha_after.
    if let (Some(before), Some(after)) = (&record.git_sha_before, &record.git_sha_after) {
        if before != after {
            if let Ok(repo) = Repository::discover(&record.cwd) {
                match crate::git_history::diff_files(&repo, before, after) {
                    Ok(file_diffs) => {
                        let (added, removed, files_changed) =
                            crate::git_history::aggregate_totals(&file_diffs);
                        record.file_diffs = file_diffs;
                        record.lines_added = added;
                        record.lines_removed = removed;
                        record.files_changed = files_changed;
                    }
                    Err(err) => eprintln!("[claude-time] diff_files failed: {err:#}"),
                }
            }
        }
    }
}

fn read_git_head(cwd: &str) -> Option<(String, String)> {
    if cwd.is_empty() {
        return None;
    }
    let repo = Repository::discover(cwd).ok()?;
    let head = repo.head().ok()?;
    let sha = head.target()?.to_string();
    let branch = head.shorthand().unwrap_or("HEAD").to_string();
    Some((sha, branch))
}
