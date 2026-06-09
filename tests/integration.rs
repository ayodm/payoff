//! End-to-end integration: install → hook events fed via stdin → report.
//!
//! Each test isolates state via `CLAUDE_CONFIG_DIR` pointing at a tempdir
//! and a synthetic git repo for the session's `cwd`.

use assert_cmd::Command;
use predicates::str::contains;
use serde_json::json;
use std::process::Stdio;
use tempfile::TempDir;

fn bin(config_dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("claude-time").unwrap();
    cmd.env("CLAUDE_CONFIG_DIR", config_dir);
    cmd
}

fn init_git_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    std::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .current_dir(dir.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.email", "t@t"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "t"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    std::fs::write(dir.path().join("seed.txt"), "seed\n").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-q", "-m", "seed"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    dir
}

#[test]
fn install_status_uninstall_roundtrip() {
    let config = TempDir::new().unwrap();

    bin(config.path()).arg("install").assert().success();
    bin(config.path())
        .arg("status")
        .assert()
        .success()
        .stdout(contains("hooks installed:  2 / 2"));

    // Idempotent re-install.
    bin(config.path())
        .arg("install")
        .assert()
        .success()
        .stdout(contains("already installed"));

    bin(config.path()).arg("uninstall").assert().success();
    bin(config.path())
        .arg("status")
        .assert()
        .success()
        .stdout(contains("hooks installed:  0 / 2"));
}

#[test]
fn install_migrates_legacy_flat_entries() {
    // Reproduces the v0.1.x bug: settings.json has our hooks in the flat
    // shape `{type, command}` that /doctor flags as invalid. Re-running
    // install must rewrite them as the wrapped shape Claude Code expects,
    // without adding duplicates.
    let config = TempDir::new().unwrap();
    let settings = config.path().join("settings.json");
    let legacy = json!({
        "hooks": {
            "SessionStart": [
                { "type": "command", "command": "claude-time hook session-start" }
            ],
            "SessionEnd": [
                { "type": "command", "command": "claude-time hook session-end" }
            ]
        }
    });
    std::fs::write(&settings, serde_json::to_string(&legacy).unwrap()).unwrap();

    bin(config.path())
        .arg("install")
        .assert()
        .success()
        .stdout(contains("Migrated 2 legacy hook entries"));

    let raw = std::fs::read_to_string(&settings).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();

    // Each event has exactly one entry, and that entry is a hook group with
    // our command inside `hooks: [...]` — the shape /doctor accepts.
    for (event, command) in [
        ("SessionStart", "claude-time hook session-start"),
        ("SessionEnd", "claude-time hook session-end"),
    ] {
        let arr = v["hooks"][event].as_array().unwrap();
        assert_eq!(arr.len(), 1, "{event} has exactly one entry");
        assert!(arr[0].get("command").is_none(), "{event} entry is not flat");
        let inner = arr[0]["hooks"].as_array().unwrap();
        assert_eq!(inner.len(), 1);
        assert_eq!(inner[0]["command"], command);
        assert_eq!(inner[0]["type"], "command");
    }
}

#[test]
fn install_preserves_unrelated_user_hooks() {
    let config = TempDir::new().unwrap();
    let settings = config.path().join("settings.json");
    let user = json!({
        "hooks": {
            "UserPromptSubmit": [
                { "type": "command", "command": "my-tool" }
            ]
        }
    });
    std::fs::write(&settings, serde_json::to_string(&user).unwrap()).unwrap();

    bin(config.path()).arg("install").assert().success();
    let raw = std::fs::read_to_string(&settings).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["hooks"]["UserPromptSubmit"][0]["command"], "my-tool");
    assert!(v["hooks"]["SessionStart"].is_array());
    assert!(v["hooks"]["SessionEnd"].is_array());

    bin(config.path()).arg("uninstall").assert().success();
    let raw = std::fs::read_to_string(&settings).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    // User's hook intact, ours gone.
    assert_eq!(v["hooks"]["UserPromptSubmit"][0]["command"], "my-tool");
    assert!(v["hooks"]["SessionStart"].is_null() || v["hooks"]["SessionStart"].as_array().unwrap().is_empty());
}

#[test]
fn end_to_end_capture_and_report() {
    let config = TempDir::new().unwrap();
    let repo = init_git_repo();

    // Empty transcript file so SessionEnd's parser has something to read.
    let transcript = config.path().join("session-transcript.jsonl");
    std::fs::write(&transcript, "").unwrap();

    let start_payload = json!({
        "session_id": "abc12345-test",
        "cwd": repo.path().to_string_lossy(),
        "transcript_path": transcript.to_string_lossy(),
        "model": "claude-opus-4-7",
    });
    bin(config.path())
        .args(["hook", "session-start"])
        .write_stdin(start_payload.to_string())
        .assert()
        .success();

    // Simulate the session adding a file and committing.
    let added = repo.path().join("added.rs");
    std::fs::write(&added, "fn one() {}\nfn two() {}\nfn three() {}\n").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(repo.path())
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-q", "-m", "session"])
        .current_dir(repo.path())
        .status()
        .unwrap();

    let end_payload = json!({
        "session_id": "abc12345-test",
        "cwd": repo.path().to_string_lossy(),
        "transcript_path": transcript.to_string_lossy(),
    });
    bin(config.path())
        .args(["hook", "session-end"])
        .write_stdin(end_payload.to_string())
        .assert()
        .success();

    // Session file should exist with both timestamps + diff stats.
    let sessions_dir = config.path().join("claude-time").join("sessions");
    let entries: Vec<_> = std::fs::read_dir(&sessions_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1);
    let raw = std::fs::read_to_string(entries[0].path()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(v["started_at"].is_string());
    assert!(v["ended_at"].is_string());
    assert!(v["git_sha_before"].is_string());
    assert!(v["git_sha_after"].is_string());
    assert_eq!(v["lines_added"].as_u64().unwrap(), 3);

    // HTML report (default): writes to <data_dir>/last-report.html and
    // tells the user the path. --no-open suppresses browser auto-open in CI.
    bin(config.path())
        .args(["report", "--since", "1d", "--no-open"])
        .assert()
        .success()
        .stdout(contains("last-report.html"));
    let html_path = config.path().join("claude-time").join("last-report.html");
    let html = std::fs::read_to_string(&html_path).unwrap();
    assert!(html.contains("<!DOCTYPE html>"));
    assert!(html.contains("Where time was wasted")); // pinpoint section
    assert!(html.contains("Quadrant"));
    assert!(html.contains("htmx.org"));

    // --stdout pipes HTML directly (CI-friendly).
    bin(config.path())
        .args(["report", "--since", "1d", "--stdout"])
        .assert()
        .success()
        .stdout(contains("<!DOCTYPE html>"));

    // --markdown keeps the terminal-readable legacy path.
    bin(config.path())
        .args(["report", "--since", "1d", "--markdown"])
        .assert()
        .success()
        .stdout(contains("## Quadrant"))
        .stdout(contains("Lines added"));
}

#[test]
fn archive_command_compacts_old_sessions() {
    let config = TempDir::new().unwrap();

    // Pre-seed a session JSON dated long ago.
    let sessions = config.path().join("claude-time").join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let record = json!({
        "session_id": "old1",
        "cwd": "/whatever",
        "started_at": "2020-01-01T00:00:00Z",
        "ended_at": "2020-01-01T00:05:00Z"
    });
    std::fs::write(
        sessions.join("old1.json"),
        serde_json::to_string(&record).unwrap(),
    )
    .unwrap();

    bin(config.path())
        .arg("archive")
        .assert()
        .success()
        .stdout(contains("Archived 1 session"));

    assert!(!sessions.join("old1.json").exists());
    let archive = config.path().join("claude-time").join("archive.jsonl");
    assert!(archive.exists());
    let contents = std::fs::read_to_string(&archive).unwrap();
    assert!(contents.contains("\"session_id\":\"old1\""));
}
