//! Install / uninstall / status — non-destructive merge into Claude Code's
//! `settings.json`.
//!
//! Idempotent: rerunning install never duplicates entries. Uninstall removes
//! only the hook entries that match our exact command, leaving any other
//! user-configured hooks intact.
//!
//! Hook shape: Claude Code expects each event array to contain *hook groups*
//! of the form `{ "hooks": [{ "type": "command", "command": "..." }] }`.
//! v0.1.x of this crate wrote the flat shape `{ "type": "command", "command":
//! "..." }` directly, which `/doctor` now flags. Install detects that legacy
//! shape and migrates it in-place; uninstall accepts both shapes so users on
//! either install can cleanly remove us.

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use std::fs;

const HOOK_EVENTS: &[(&str, &str)] = &[
    ("SessionStart", "claude-time hook session-start"),
    ("SessionEnd", "claude-time hook session-end"),
];

pub fn install() -> Result<()> {
    crate::paths::ensure_dirs()?;
    let settings_path = crate::paths::settings_json()?;

    if let Some(parent) = settings_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut root = read_or_default(&settings_path)?;
    let (added, migrated) = apply_hooks(&mut root)?;

    let serialized = serde_json::to_string_pretty(&root)?;
    fs::write(&settings_path, serialized)
        .with_context(|| format!("writing {}", settings_path.display()))?;

    match (added, migrated) {
        (0, 0) => println!(
            "claude-time hooks already installed; no changes to settings.json."
        ),
        (a, 0) => println!(
            "Installed {a} hook entries in {}.",
            settings_path.display()
        ),
        (0, m) => println!(
            "Migrated {m} legacy hook entries to the current shape in {}.",
            settings_path.display()
        ),
        (a, m) => println!(
            "Installed {a} hook entries and migrated {m} legacy entries in {}.",
            settings_path.display()
        ),
    }
    println!("Data dir: {}", crate::paths::data_dir()?.display());
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let settings_path = crate::paths::settings_json()?;
    if !settings_path.exists() {
        println!("No settings.json at {}; nothing to do.", settings_path.display());
        return Ok(());
    }
    let mut root = read_or_default(&settings_path)?;
    let removed = remove_hooks(&mut root);

    let serialized = serde_json::to_string_pretty(&root)?;
    fs::write(&settings_path, serialized)?;

    println!(
        "Removed {removed} claude-time hook entries from {}.",
        settings_path.display()
    );
    println!(
        "Data dir left intact at {}. Delete it manually if you want a clean slate.",
        crate::paths::data_dir()?.display()
    );
    Ok(())
}

pub fn status() -> Result<()> {
    let settings_path = crate::paths::settings_json()?;
    let installed = match read_or_default(&settings_path) {
        Ok(v) => count_our_hooks(&v),
        Err(_) => 0,
    };

    let sessions_dir = crate::paths::sessions_dir()?;
    let session_count = if sessions_dir.exists() {
        fs::read_dir(&sessions_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("json"))
            .count()
    } else {
        0
    };

    println!("settings.json:    {}", settings_path.display());
    println!("hooks installed:  {installed} / {}", HOOK_EVENTS.len());
    println!("sessions dir:     {}", sessions_dir.display());
    println!("sessions stored:  {session_count}");
    Ok(())
}

fn read_or_default(path: &std::path::Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(json!({}));
    }
    let v: Value = serde_json::from_str(&raw)
        .with_context(|| format!("parsing {}", path.display()))?;
    Ok(v)
}

/// Insert our hook entries; return `(added, migrated)`.
///
/// `added` counts brand-new wrapped entries appended. `migrated` counts
/// legacy flat entries rewritten in place to the wrapped shape.
fn apply_hooks(root: &mut Value) -> Result<(usize, usize)> {
    let obj = ensure_object(root)?;
    let hooks_entry = obj.entry("hooks").or_insert(Value::Object(Map::new()));
    let hooks_obj = match hooks_entry.as_object_mut() {
        Some(o) => o,
        None => anyhow::bail!("`hooks` in settings.json is not an object"),
    };

    let mut added = 0usize;
    let mut migrated = 0usize;
    for (event, command) in HOOK_EVENTS {
        let arr = hooks_obj
            .entry(*event)
            .or_insert(Value::Array(Vec::new()));
        let arr = match arr.as_array_mut() {
            Some(a) => a,
            None => anyhow::bail!("`hooks.{event}` is not an array"),
        };

        // Two-pass: scan first so we can pick the right migration strategy,
        // then mutate. The goal is "exactly one wrapped entry of ours in this
        // event array" — any other configuration is collapsed toward that.
        let has_wrapped = arr.iter().any(|i| is_wrapped_match(i, command));
        let flat_count = arr.iter().filter(|i| is_flat_match(i, command)).count();

        if has_wrapped {
            // Wrapped already correct — drop any leftover flat entries from a
            // partially-applied prior install.
            if flat_count > 0 {
                arr.retain(|i| !is_flat_match(i, command));
                migrated += flat_count;
            }
        } else if flat_count > 0 {
            // Rewrite the first flat entry in place, drop the rest. This
            // preserves the entry's position in the array while collapsing
            // accidental duplicates.
            let mut rewritten = false;
            arr.retain_mut(|item| {
                if is_flat_match(item, command) {
                    if !rewritten {
                        *item = wrapped_entry(command);
                        rewritten = true;
                        true
                    } else {
                        false
                    }
                } else {
                    true
                }
            });
            migrated += flat_count;
        } else {
            arr.push(wrapped_entry(command));
            added += 1;
        }
    }
    Ok((added, migrated))
}

/// Remove our hook entries; return how many commands were removed.
///
/// Accepts both shapes:
///   * top-level flat entries `{type, command}` written by v0.1.x
///   * wrapped entries `{hooks: [{type, command}]}` written by ≥v0.1.2
fn remove_hooks(root: &mut Value) -> usize {
    let Some(obj) = root.as_object_mut() else { return 0 };
    let Some(hooks_entry) = obj.get_mut("hooks") else { return 0 };
    let Some(hooks_obj) = hooks_entry.as_object_mut() else { return 0 };

    let mut removed = 0usize;
    for (event, command) in HOOK_EVENTS {
        if let Some(arr_value) = hooks_obj.get_mut(*event) {
            if let Some(arr) = arr_value.as_array_mut() {
                let before_flat = arr.len();
                arr.retain(|item| !is_flat_match(item, command));
                removed += before_flat - arr.len();

                for item in arr.iter_mut() {
                    if let Some(inner) = item
                        .get_mut("hooks")
                        .and_then(|h| h.as_array_mut())
                    {
                        let before_inner = inner.len();
                        inner.retain(|e| !is_flat_match(e, command));
                        removed += before_inner - inner.len();
                    }
                }
                arr.retain(|item| match item.get("hooks").and_then(|h| h.as_array()) {
                    Some(inner) => !inner.is_empty(),
                    None => true,
                });
            }
        }
    }
    // Drop empty arrays so settings.json stays tidy.
    for (event, _) in HOOK_EVENTS {
        if let Some(Value::Array(a)) = hooks_obj.get(*event) {
            if a.is_empty() {
                hooks_obj.remove(*event);
            }
        }
    }
    if hooks_obj.is_empty() {
        obj.remove("hooks");
    }
    removed
}

fn count_our_hooks(root: &Value) -> usize {
    let Some(hooks) = root.get("hooks").and_then(|h| h.as_object()) else { return 0 };
    HOOK_EVENTS
        .iter()
        .filter(|(event, command)| {
            hooks
                .get(*event)
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter().any(|item| {
                        is_flat_match(item, command) || is_wrapped_match(item, command)
                    })
                })
                .unwrap_or(false)
        })
        .count()
}

fn wrapped_entry(command: &str) -> Value {
    json!({
        "hooks": [
            { "type": "command", "command": command }
        ]
    })
}

fn is_flat_match(item: &Value, command: &str) -> bool {
    item.get("command").and_then(|c| c.as_str()) == Some(command)
}

fn is_wrapped_match(item: &Value, command: &str) -> bool {
    item.get("hooks")
        .and_then(|h| h.as_array())
        .map(|inner| inner.iter().any(|e| is_flat_match(e, command)))
        .unwrap_or(false)
}

fn ensure_object(v: &mut Value) -> Result<&mut Map<String, Value>> {
    if !v.is_object() {
        anyhow::bail!("settings.json root is not a JSON object");
    }
    Ok(v.as_object_mut().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_root() -> Value {
        json!({})
    }

    fn our_command_present(arr: &[Value], command: &str) -> bool {
        arr.iter()
            .any(|item| is_flat_match(item, command) || is_wrapped_match(item, command))
    }

    #[test]
    fn install_adds_two_entries_in_empty_settings() {
        let mut root = fresh_root();
        let (added, migrated) = apply_hooks(&mut root).unwrap();
        assert_eq!(added, 2);
        assert_eq!(migrated, 0);
        assert_eq!(count_our_hooks(&root), 2);
    }

    #[test]
    fn install_writes_wrapped_shape() {
        let mut root = fresh_root();
        apply_hooks(&mut root).unwrap();
        let session_start = root["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(session_start.len(), 1);
        let entry = &session_start[0];
        // Outer entry has only `hooks`, not a top-level `command`.
        assert!(entry.get("command").is_none());
        let inner = entry["hooks"].as_array().unwrap();
        assert_eq!(inner.len(), 1);
        assert_eq!(inner[0]["type"], "command");
        assert_eq!(inner[0]["command"], "claude-time hook session-start");
    }

    #[test]
    fn install_is_idempotent() {
        let mut root = fresh_root();
        apply_hooks(&mut root).unwrap();
        let (added, migrated) = apply_hooks(&mut root).unwrap();
        assert_eq!(added, 0);
        assert_eq!(migrated, 0);
        assert_eq!(count_our_hooks(&root), 2);
    }

    #[test]
    fn install_migrates_legacy_flat_entry() {
        // Simulate a settings.json written by v0.1.x: our command sitting as a
        // top-level flat entry alongside a user-defined wrapped block.
        let mut root = json!({
            "hooks": {
                "SessionStart": [
                    { "type": "command", "command": "claude-time hook session-start" },
                    { "hooks": [{ "type": "command", "command": "user-tool" }] }
                ],
                "SessionEnd": [
                    { "type": "command", "command": "claude-time hook session-end" }
                ]
            }
        });
        let (added, migrated) = apply_hooks(&mut root).unwrap();
        assert_eq!(added, 0, "no new entries — both already present in flat shape");
        assert_eq!(migrated, 2, "both flat entries rewritten as wrapped");

        // Our SessionStart entry is now wrapped, and the user's entry survived.
        let ss = root["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(ss.len(), 2);
        assert!(ss
            .iter()
            .any(|i| is_wrapped_match(i, "claude-time hook session-start")));
        assert!(ss
            .iter()
            .any(|i| is_wrapped_match(i, "user-tool")));
        // No flat entry of ours remains.
        assert!(!ss
            .iter()
            .any(|i| is_flat_match(i, "claude-time hook session-start")));

        // Status counts the migrated entries.
        assert_eq!(count_our_hooks(&root), 2);
    }

    #[test]
    fn install_dedupes_half_migrated_state() {
        // A previous install attempt could have left both a legacy flat entry
        // and a freshly-added wrapped entry side by side. apply_hooks should
        // collapse this to one wrapped entry, not produce two.
        let mut root = json!({
            "hooks": {
                "SessionStart": [
                    { "type": "command", "command": "claude-time hook session-start" },
                    { "hooks": [{ "type": "command", "command": "claude-time hook session-start" }] }
                ],
                "SessionEnd": [
                    { "hooks": [{ "type": "command", "command": "claude-time hook session-end" }] }
                ]
            }
        });
        let (added, migrated) = apply_hooks(&mut root).unwrap();
        assert_eq!(added, 0);
        assert_eq!(migrated, 1, "the leftover flat entry counts as cleaned up");
        let ss = root["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(ss.len(), 1, "duplicates collapsed");
        assert!(is_wrapped_match(&ss[0], "claude-time hook session-start"));
    }

    #[test]
    fn install_collapses_duplicate_flat_entries() {
        // Two flat copies of our command — rewrite the first to wrapped, drop
        // the second. Net: one wrapped entry.
        let mut root = json!({
            "hooks": {
                "SessionStart": [
                    { "type": "command", "command": "claude-time hook session-start" },
                    { "type": "command", "command": "claude-time hook session-start" }
                ],
                "SessionEnd": [
                    { "hooks": [{ "type": "command", "command": "claude-time hook session-end" }] }
                ]
            }
        });
        let (added, migrated) = apply_hooks(&mut root).unwrap();
        assert_eq!(added, 0);
        assert_eq!(migrated, 2);
        let ss = root["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(ss.len(), 1);
        assert!(is_wrapped_match(&ss[0], "claude-time hook session-start"));
    }

    #[test]
    fn install_after_migration_is_idempotent() {
        // Re-running install on already-migrated settings is a no-op.
        let mut root = json!({
            "hooks": {
                "SessionStart": [
                    { "type": "command", "command": "claude-time hook session-start" }
                ]
            }
        });
        let (_, migrated_first) = apply_hooks(&mut root).unwrap();
        assert_eq!(migrated_first, 1);
        let (added, migrated) = apply_hooks(&mut root).unwrap();
        assert_eq!(added, 0);
        assert_eq!(migrated, 0);
    }

    #[test]
    fn install_preserves_user_hooks() {
        let mut root = json!({
            "hooks": {
                "SessionStart": [
                    { "hooks": [{ "type": "command", "command": "my-existing-hook" }] }
                ],
                "UserPromptSubmit": [
                    { "hooks": [{ "type": "command", "command": "another-tool" }] }
                ]
            }
        });
        apply_hooks(&mut root).unwrap();

        let session_start = root["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(session_start.len(), 2, "user hook + ours");
        assert!(our_command_present(session_start, "my-existing-hook"));
        assert!(our_command_present(
            session_start,
            "claude-time hook session-start"
        ));

        // Unrelated hook events untouched.
        assert!(root["hooks"]["UserPromptSubmit"].is_array());
    }

    #[test]
    fn uninstall_removes_wrapped_entries() {
        let mut root = fresh_root();
        apply_hooks(&mut root).unwrap();
        // Add a user hook alongside, wrapped (modern shape).
        root["hooks"]["SessionStart"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "hooks": [{ "type": "command", "command": "user-hook" }] }));

        let removed = remove_hooks(&mut root);
        assert_eq!(removed, 2);
        let session_start = root["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(session_start.len(), 1);
        assert!(is_wrapped_match(&session_start[0], "user-hook"));
    }

    #[test]
    fn uninstall_removes_legacy_flat_entries() {
        // Users who installed v0.1.x have flat entries — uninstall must clear
        // them too, or the upgrade path leaves broken hooks behind.
        let mut root = json!({
            "hooks": {
                "SessionStart": [
                    { "type": "command", "command": "claude-time hook session-start" }
                ],
                "SessionEnd": [
                    { "type": "command", "command": "claude-time hook session-end" }
                ]
            }
        });
        let removed = remove_hooks(&mut root);
        assert_eq!(removed, 2);
        assert!(root.get("hooks").is_none(), "empty hooks block pruned");
    }

    #[test]
    fn uninstall_strips_our_command_from_shared_wrapped_entry() {
        // A wrapped entry that holds both our command and a user command —
        // strip ours, keep the rest, don't drop the wrapper.
        let mut root = json!({
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            { "type": "command", "command": "claude-time hook session-start" },
                            { "type": "command", "command": "user-tool" }
                        ]
                    }
                ]
            }
        });
        let removed = remove_hooks(&mut root);
        assert_eq!(removed, 1);
        let ss = root["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(ss.len(), 1);
        let inner = ss[0]["hooks"].as_array().unwrap();
        assert_eq!(inner.len(), 1);
        assert_eq!(inner[0]["command"], "user-tool");
    }

    #[test]
    fn uninstall_cleans_up_empty_hook_keys() {
        let mut root = fresh_root();
        apply_hooks(&mut root).unwrap();
        remove_hooks(&mut root);
        // Hooks key should be gone entirely since nothing else was there.
        assert!(root.get("hooks").is_none());
    }

    #[test]
    fn count_our_hooks_recognises_both_shapes() {
        let root = json!({
            "hooks": {
                "SessionStart": [
                    { "type": "command", "command": "claude-time hook session-start" }
                ],
                "SessionEnd": [
                    { "hooks": [{ "type": "command", "command": "claude-time hook session-end" }] }
                ]
            }
        });
        assert_eq!(count_our_hooks(&root), 2);
    }
}
