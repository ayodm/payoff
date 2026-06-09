//! Realistic transcript-parse tests beyond the basics covered in
//! src/transcript.rs::tests. Each test names exactly what behavior is
//! checked and the hand-computed expected values.

use claude_time::transcript::parse;
use serde_json::{json, Value};
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
fn realistic_multi_turn_session_aggregates_match_hand_compute() {
    // A plausible session shape: 3 user turns, interleaved assistant
    // responses with tool calls and usage blocks.
    let f = jsonl(&[
        // Turn 1
        json!({"message": {"role": "user", "content": [{"type": "text", "text": "Read foo.rs"}]}}),
        json!({
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "I'll read the file."},
                    {"type": "tool_use", "name": "Read", "input": {"file_path": "/repo/foo.rs"}}
                ],
                "usage": {"input_tokens": 1000, "output_tokens": 50, "cache_read_input_tokens": 500, "cache_creation_input_tokens": 100}
            },
            "costUSD": 0.015
        }),
        // Tool result — NOT a turn
        json!({"message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "x"}]}}),

        // Turn 2
        json!({"message": {"role": "user", "content": "Make the change"}}),
        json!({
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "tool_use", "name": "Edit", "input": {"file_path": "/repo/foo.rs"}},
                    {"type": "tool_use", "name": "Edit", "input": {"file_path": "/repo/foo.rs"}},
                    {"type": "tool_use", "name": "Bash", "input": {"command": "cargo test"}}
                ],
                "usage": {"input_tokens": 2000, "output_tokens": 100}
            },
            "costUSD": 0.030
        }),
        json!({"message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "y"}]}}),

        // Turn 3 — also writes a new file
        json!({"message": {"role": "user", "content": "Now add bar.rs"}}),
        json!({
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "tool_use", "name": "Write", "input": {"file_path": "/repo/bar.rs"}}
                ],
                "usage": {"input_tokens": 1500, "output_tokens": 200, "cache_read_input_tokens": 1000}
            },
            "costUSD": 0.020
        }),
    ]);

    let s = parse(f.path()).unwrap();

    // Hand-computed expectations:
    // - 3 user turns (not counting tool_results)
    // - Tool calls: Read=1, Edit=2, Bash=1, Write=1 (4 distinct names)
    // - files_modified: foo.rs and bar.rs (Read doesn't count — not in FILE_EDIT_TOOLS)
    // - per_file_edits: foo.rs=2 (two Edits), bar.rs=1 (one Write)
    // - Tokens: input=4500, output=350, cache_read=1500, cache_creation=100
    // - Cost: 0.015 + 0.030 + 0.020 = 0.065
    assert_eq!(s.turn_count, 3, "user turns");
    assert_eq!(s.tool_calls.get("Read").copied(), Some(1));
    assert_eq!(s.tool_calls.get("Edit").copied(), Some(2));
    assert_eq!(s.tool_calls.get("Bash").copied(), Some(1));
    assert_eq!(s.tool_calls.get("Write").copied(), Some(1));
    assert_eq!(s.tool_calls.len(), 4);
    assert_eq!(s.files_modified, vec!["/repo/bar.rs", "/repo/foo.rs"]);
    assert_eq!(s.per_file_edits.get("/repo/foo.rs").copied(), Some(2));
    assert_eq!(s.per_file_edits.get("/repo/bar.rs").copied(), Some(1));
    assert_eq!(s.input_tokens, 4500);
    assert_eq!(s.output_tokens, 350);
    assert_eq!(s.cache_read_tokens, 1500);
    assert_eq!(s.cache_creation_tokens, 100);
    assert!((s.total_cost_usd - 0.065).abs() < 1e-9, "got {}", s.total_cost_usd);
}

#[test]
fn user_message_with_string_content_still_counts_as_turn() {
    // Claude Code sometimes emits user content as a plain string instead of
    // an array of content blocks. The turn should still count.
    let f = jsonl(&[
        json!({"message": {"role": "user", "content": "Hello there"}}),
        json!({"message": {"role": "user", "content": "Another"}}),
    ]);
    let s = parse(f.path()).unwrap();
    assert_eq!(s.turn_count, 2);
}

#[test]
fn tool_use_without_name_does_not_crash_or_count() {
    // Defensive: a tool_use block missing the `name` field shouldn't crash
    // the parser and shouldn't increment any tool_calls bucket.
    let f = jsonl(&[
        json!({
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "tool_use", "input": {"file_path": "/a"}},  // no name
                    {"type": "tool_use", "name": "Edit", "input": {"file_path": "/b"}}
                ]
            }
        }),
    ]);
    let s = parse(f.path()).unwrap();
    assert_eq!(s.tool_calls.len(), 1);
    assert_eq!(s.tool_calls.get("Edit").copied(), Some(1));
    assert_eq!(s.files_modified, vec!["/b"]);
}

#[test]
fn cost_as_integer_is_accepted() {
    // serde_json::Value::as_f64 accepts integers — verify.
    let f = jsonl(&[
        json!({"costUSD": 1, "message": {"role": "assistant"}}),
        json!({"cost_usd": 2, "message": {"role": "assistant"}}),
    ]);
    let s = parse(f.path()).unwrap();
    assert!((s.total_cost_usd - 3.0).abs() < 1e-9, "got {}", s.total_cost_usd);
}

#[test]
fn both_cost_field_names_in_same_entry_costusd_wins() {
    // The parser uses `.or_else()` — costUSD wins; cost_usd is fallback.
    // Document this so a future PR doesn't accidentally double-count.
    let f = jsonl(&[
        json!({"costUSD": 0.10, "cost_usd": 99.99, "message": {"role": "assistant"}}),
    ]);
    let s = parse(f.path()).unwrap();
    assert!((s.total_cost_usd - 0.10).abs() < 1e-9, "costUSD wins; got {}", s.total_cost_usd);
}

#[test]
fn multiedit_only_captures_top_level_file_path_field() {
    // Claude Code's real MultiEdit shape uses `input.file_path` and an
    // `edits` array. The parser only looks at `input.file_path`, so a
    // single MultiEdit call counts as ONE edit even if it contains many
    // sub-edits. Document so the pinpoint logic isn't misread.
    let f = jsonl(&[
        json!({
            "message": {
                "role": "assistant",
                "content": [
                    {
                        "type": "tool_use",
                        "name": "MultiEdit",
                        "input": {
                            "file_path": "/repo/big.rs",
                            "edits": [
                                {"old_string": "a", "new_string": "b"},
                                {"old_string": "c", "new_string": "d"},
                                {"old_string": "e", "new_string": "f"}
                            ]
                        }
                    }
                ]
            }
        }),
    ]);
    let s = parse(f.path()).unwrap();
    assert_eq!(s.tool_calls.get("MultiEdit").copied(), Some(1));
    assert_eq!(
        s.per_file_edits.get("/repo/big.rs").copied(),
        Some(1),
        "MultiEdit = 1 edit in per_file_edits, regardless of inner edits count"
    );
}

#[test]
fn empty_transcript_returns_zero_stats() {
    let f = NamedTempFile::new().unwrap();
    let s = parse(f.path()).unwrap();
    assert_eq!(s.turn_count, 0);
    assert!(s.tool_calls.is_empty());
    assert!(s.files_modified.is_empty());
    assert!(s.per_file_edits.is_empty());
    assert_eq!(s.input_tokens, 0);
    assert_eq!(s.total_cost_usd, 0.0);
}

#[test]
fn very_large_transcript_parses_in_reasonable_time() {
    // 10k user-turn entries. Confirms streaming works (no O(n²) blowup)
    // and the parser handles realistic session sizes.
    let mut f = NamedTempFile::new().unwrap();
    for _ in 0..10_000 {
        writeln!(
            f,
            r#"{{"message":{{"role":"user","content":[]}}}}"#
        )
        .unwrap();
    }
    let start = std::time::Instant::now();
    let s = parse(f.path()).unwrap();
    let elapsed = start.elapsed();
    assert_eq!(s.turn_count, 10_000);
    eprintln!("parsed 10k entries in {elapsed:?}");
    assert!(elapsed.as_millis() < 1000, "parsing took >1s: {elapsed:?}");
}

#[test]
fn line_with_unicode_paths_preserves_them() {
    // Filenames with unicode and spaces must round-trip cleanly.
    let f = jsonl(&[
        json!({
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "tool_use", "name": "Edit", "input": {"file_path": "/repo/日本語/ファイル名.rs"}},
                    {"type": "tool_use", "name": "Write", "input": {"file_path": "/repo/path with spaces/file.txt"}}
                ]
            }
        }),
    ]);
    let s = parse(f.path()).unwrap();
    assert!(s.files_modified.contains(&"/repo/日本語/ファイル名.rs".to_string()));
    assert!(s.files_modified.contains(&"/repo/path with spaces/file.txt".to_string()));
    assert_eq!(
        s.per_file_edits.get("/repo/日本語/ファイル名.rs").copied(),
        Some(1)
    );
}

#[test]
fn missing_usage_field_is_zero_not_error() {
    // An assistant message without a `usage` block must not affect tokens.
    let f = jsonl(&[
        json!({"message": {"role": "assistant", "content": []}}),
        json!({"message": {"role": "assistant", "usage": {"input_tokens": 10}}}),
    ]);
    let s = parse(f.path()).unwrap();
    assert_eq!(s.input_tokens, 10);
}
