//! Streaming JSONL parser for Claude Code session transcripts.
//!
//! The transcript format is not formally documented, so we parse defensively:
//! each line becomes a `serde_json::Value` and we extract only the fields we
//! recognize. Unknown line shapes are skipped silently. This trades some
//! schema rigor for forward-compat as Claude Code's transcript format evolves.

use anyhow::Result;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

#[derive(Debug, Default)]
pub struct TranscriptStats {
    pub turn_count: u32,
    pub tool_calls: BTreeMap<String, u32>,
    pub files_modified: Vec<String>,
    pub total_cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
}

/// Tool names whose `input.file_path` we want to capture as "files modified."
const FILE_EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];

pub fn parse(path: &Path) -> Result<TranscriptStats> {
    let mut stats = TranscriptStats::default();
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut files_set: std::collections::BTreeSet<String> = Default::default();

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue, // tolerate partial lines
        };
        ingest_entry(&value, &mut stats, &mut files_set);
    }

    stats.files_modified = files_set.into_iter().collect();
    Ok(stats)
}

fn ingest_entry(
    value: &Value,
    stats: &mut TranscriptStats,
    files_set: &mut std::collections::BTreeSet<String>,
) {
    // Count user-role turns. Tool results carry role "user" but aren't real
    // turns — Claude Code wraps them with a `tool_use_id` field or marks
    // `type: "tool_result"`. Filter those out.
    if let Some(role) = value
        .get("message")
        .and_then(|m| m.get("role"))
        .and_then(|r| r.as_str())
    {
        if role == "user" && !is_tool_result(value) {
            stats.turn_count += 1;
        }
    }

    // Cost/tokens land on assistant-message entries under `message.usage`
    // and a top-level `costUSD` (or `cost_usd`).
    if let Some(usage) = value
        .get("message")
        .and_then(|m| m.get("usage"))
        .and_then(|u| u.as_object())
    {
        stats.input_tokens += as_u64(usage.get("input_tokens"));
        stats.output_tokens += as_u64(usage.get("output_tokens"));
        stats.cache_read_tokens += as_u64(usage.get("cache_read_input_tokens"));
        stats.cache_creation_tokens += as_u64(usage.get("cache_creation_input_tokens"));
    }
    if let Some(cost) = value
        .get("costUSD")
        .or_else(|| value.get("cost_usd"))
        .and_then(|c| c.as_f64())
    {
        stats.total_cost_usd += cost;
    }

    // Tool uses appear as content blocks of type "tool_use" inside the
    // assistant message body.
    if let Some(content) = value
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    {
        for block in content {
            if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                if let Some(name) = block.get("name").and_then(|n| n.as_str()) {
                    *stats.tool_calls.entry(name.to_string()).or_default() += 1;
                    if FILE_EDIT_TOOLS.contains(&name) {
                        if let Some(path) = block
                            .get("input")
                            .and_then(|i| i.get("file_path"))
                            .and_then(|p| p.as_str())
                        {
                            files_set.insert(path.to_string());
                        }
                    }
                }
            }
        }
    }
}

fn is_tool_result(value: &Value) -> bool {
    if value.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
        return true;
    }
    if let Some(content) = value
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    {
        return content
            .iter()
            .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"));
    }
    false
}

fn as_u64(v: Option<&Value>) -> u64 {
    v.and_then(|x| x.as_u64()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn jsonl(lines: &[Value]) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        for v in lines {
            writeln!(f, "{}", serde_json::to_string(v).unwrap()).unwrap();
        }
        f
    }

    #[test]
    fn counts_user_turns_but_not_tool_results() {
        let f = jsonl(&[
            serde_json::json!({"message": {"role": "user", "content": []}}),
            serde_json::json!({"message": {"role": "assistant", "content": []}}),
            serde_json::json!({"message": {"role": "user", "content": [{"type": "tool_result"}]}}),
            serde_json::json!({"message": {"role": "user", "content": []}}),
        ]);
        let stats = parse(f.path()).unwrap();
        assert_eq!(stats.turn_count, 2);
    }

    #[test]
    fn accumulates_token_usage() {
        let f = jsonl(&[
            serde_json::json!({
                "message": {
                    "role": "assistant",
                    "usage": {
                        "input_tokens": 100,
                        "output_tokens": 50,
                        "cache_read_input_tokens": 200,
                        "cache_creation_input_tokens": 25
                    }
                }
            }),
            serde_json::json!({
                "message": {
                    "role": "assistant",
                    "usage": { "input_tokens": 10, "output_tokens": 5 }
                }
            }),
        ]);
        let stats = parse(f.path()).unwrap();
        assert_eq!(stats.input_tokens, 110);
        assert_eq!(stats.output_tokens, 55);
        assert_eq!(stats.cache_read_tokens, 200);
        assert_eq!(stats.cache_creation_tokens, 25);
    }

    #[test]
    fn collects_tool_calls_and_edited_files() {
        let f = jsonl(&[
            serde_json::json!({
                "message": {
                    "role": "assistant",
                    "content": [
                        {"type": "tool_use", "name": "Edit", "input": {"file_path": "/a/b.rs"}},
                        {"type": "tool_use", "name": "Bash", "input": {"command": "ls"}},
                        {"type": "tool_use", "name": "Write", "input": {"file_path": "/a/c.rs"}}
                    ]
                }
            }),
            serde_json::json!({
                "message": {
                    "role": "assistant",
                    "content": [
                        {"type": "tool_use", "name": "Edit", "input": {"file_path": "/a/b.rs"}}
                    ]
                }
            }),
        ]);
        let stats = parse(f.path()).unwrap();
        assert_eq!(stats.tool_calls.get("Edit").copied(), Some(2));
        assert_eq!(stats.tool_calls.get("Bash").copied(), Some(1));
        assert_eq!(stats.tool_calls.get("Write").copied(), Some(1));
        assert_eq!(stats.files_modified, vec!["/a/b.rs", "/a/c.rs"]);
    }

    #[test]
    fn tolerates_malformed_lines() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "{}", r#"{"message": {"role": "user", "content": []}}"#).unwrap();
        writeln!(f, "this is not json").unwrap();
        writeln!(f, "").unwrap();
        writeln!(f, "{}", r#"{"message": {"role": "user", "content": []}}"#).unwrap();
        let stats = parse(f.path()).unwrap();
        assert_eq!(stats.turn_count, 2);
    }

    #[test]
    fn sums_cost() {
        let f = jsonl(&[
            serde_json::json!({"costUSD": 0.10, "message": {"role": "assistant"}}),
            serde_json::json!({"cost_usd": 0.05, "message": {"role": "assistant"}}),
        ]);
        let stats = parse(f.path()).unwrap();
        assert!((stats.total_cost_usd - 0.15).abs() < 1e-9);
    }
}
