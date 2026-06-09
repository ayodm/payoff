//! Install / uninstall / status — non-destructive merge into Claude Code's
//! `settings.json`.
//!
//! Idempotent: rerunning install never duplicates entries. Uninstall removes
//! only the hook entries that match our exact command, leaving any other
//! user-configured hooks intact.

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
    let added = apply_hooks(&mut root)?;

    let serialized = serde_json::to_string_pretty(&root)?;
    fs::write(&settings_path, serialized)
        .with_context(|| format!("writing {}", settings_path.display()))?;

    if added == 0 {
        println!("claude-time hooks already installed; no changes to settings.json.");
    } else {
        println!(
            "Installed {added} hook entries in {}.",
            settings_path.display()
        );
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

/// Insert our hook entries; return how many were added (0 if all already present).
fn apply_hooks(root: &mut Value) -> Result<usize> {
    let obj = ensure_object(root)?;
    let hooks_entry = obj.entry("hooks").or_insert(Value::Object(Map::new()));
    let hooks_obj = match hooks_entry.as_object_mut() {
        Some(o) => o,
        None => anyhow::bail!("`hooks` in settings.json is not an object"),
    };

    let mut added = 0usize;
    for (event, command) in HOOK_EVENTS {
        let arr = hooks_obj
            .entry(*event)
            .or_insert(Value::Array(Vec::new()));
        let arr = match arr.as_array_mut() {
            Some(a) => a,
            None => anyhow::bail!("`hooks.{event}` is not an array"),
        };
        if !arr.iter().any(|item| matches_our_command(item, command)) {
            arr.push(json!({ "type": "command", "command": command }));
            added += 1;
        }
    }
    Ok(added)
}

/// Remove our hook entries; return how many were removed.
fn remove_hooks(root: &mut Value) -> usize {
    let Some(obj) = root.as_object_mut() else { return 0 };
    let Some(hooks_entry) = obj.get_mut("hooks") else { return 0 };
    let Some(hooks_obj) = hooks_entry.as_object_mut() else { return 0 };

    let mut removed = 0usize;
    for (event, command) in HOOK_EVENTS {
        if let Some(arr_value) = hooks_obj.get_mut(*event) {
            if let Some(arr) = arr_value.as_array_mut() {
                let before = arr.len();
                arr.retain(|item| !matches_our_command(item, command));
                removed += before - arr.len();
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
                .map(|arr| arr.iter().any(|item| matches_our_command(item, command)))
                .unwrap_or(false)
        })
        .count()
}

fn matches_our_command(item: &Value, command: &str) -> bool {
    item.get("command").and_then(|c| c.as_str()) == Some(command)
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

    #[test]
    fn install_adds_two_entries_in_empty_settings() {
        let mut root = fresh_root();
        let added = apply_hooks(&mut root).unwrap();
        assert_eq!(added, 2);
        assert_eq!(count_our_hooks(&root), 2);
    }

    #[test]
    fn install_is_idempotent() {
        let mut root = fresh_root();
        apply_hooks(&mut root).unwrap();
        let added = apply_hooks(&mut root).unwrap();
        assert_eq!(added, 0);
        assert_eq!(count_our_hooks(&root), 2);
    }

    #[test]
    fn install_preserves_user_hooks() {
        let mut root = json!({
            "hooks": {
                "SessionStart": [
                    { "type": "command", "command": "my-existing-hook" }
                ],
                "UserPromptSubmit": [
                    { "type": "command", "command": "another-tool" }
                ]
            }
        });
        apply_hooks(&mut root).unwrap();

        let session_start = root["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(session_start.len(), 2, "user hook + ours");
        assert!(session_start
            .iter()
            .any(|i| i["command"] == "my-existing-hook"));
        assert!(session_start
            .iter()
            .any(|i| i["command"] == "claude-time hook session-start"));

        // Unrelated hook events untouched.
        assert!(root["hooks"]["UserPromptSubmit"].is_array());
    }

    #[test]
    fn uninstall_removes_only_our_entries() {
        let mut root = fresh_root();
        apply_hooks(&mut root).unwrap();
        // Add a user hook alongside.
        root["hooks"]["SessionStart"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "type": "command", "command": "user-hook" }));

        let removed = remove_hooks(&mut root);
        assert_eq!(removed, 2);
        let session_start = root["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(session_start.len(), 1);
        assert_eq!(session_start[0]["command"], "user-hook");
    }

    #[test]
    fn uninstall_cleans_up_empty_hook_keys() {
        let mut root = fresh_root();
        apply_hooks(&mut root).unwrap();
        remove_hooks(&mut root);
        // Hooks key should be gone entirely since nothing else was there.
        assert!(root.get("hooks").is_none());
    }
}
