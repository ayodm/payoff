//! Snapshot the prompt environment at SessionStart.
//!
//! What was *available* to Claude during this session — skills, CLAUDE.md
//! rules, hooks, enabled plugins — gets recorded onto the session so the
//! correlation layer (Phase 2) can answer "did adding skill X / rewriting
//! CLAUDE.md / changing my hooks move my retention number?"
//!
//! Fail-soft by design: every read failure produces an empty `CapturedEnv`
//! rather than an error. A session record with empty driver fields is still
//! useful; an aborted hook is not.

use crate::model::{SkillRef, SkillSource};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Maximum directory levels walked upward from cwd looking for CLAUDE.md.
/// Caps the worst-case I/O on deep filesystem trees.
const CLAUDE_MD_WALK_DEPTH: usize = 8;

/// Per-event hook command prefixes we consider "ours" and exclude from
/// `active_hook_events` counts. Otherwise the correlation layer would
/// discover the meaningless pattern "every session has a SessionStart hook"
/// (because we put it there). Includes the prior tool name (`claude-time`)
/// so upgraders whose hooks haven't been re-`payoff install`ed yet still
/// see clean data.
const OUR_HOOK_COMMAND_PREFIXES: &[&str] = &["payoff hook ", "claude-time hook "];

#[derive(Debug, Default, Clone)]
pub struct CapturedEnv {
    pub active_skills: Vec<SkillRef>,
    pub claude_md_files: BTreeMap<String, String>,
    pub active_hook_events: BTreeMap<String, u32>,
    pub enabled_plugins: Vec<String>,
}

/// Snapshot the environment for a session whose cwd is `cwd`. Never errors;
/// any failure is logged to stderr and the corresponding field stays empty.
pub fn capture(cwd: &Path) -> CapturedEnv {
    capture_with_claude_dir(cwd, crate::paths::claude_config_dir().ok())
}

/// Same as [`capture`] but with an explicit `claude_dir`, so tests can
/// point at a synthetic config without mutating `CLAUDE_CONFIG_DIR`
/// (which races against other env-var-mutating tests).
pub fn capture_with_claude_dir(cwd: &Path, claude_dir: Option<PathBuf>) -> CapturedEnv {
    let settings_value = claude_dir
        .as_ref()
        .and_then(|d| read_settings_json(&d.join("settings.json")));

    let active_hook_events = settings_value
        .as_ref()
        .map(count_non_ours_hooks)
        .unwrap_or_default();

    let enabled_plugins = settings_value
        .as_ref()
        .map(enabled_plugins_from)
        .unwrap_or_default();

    let mut active_skills = Vec::new();
    if let Some(ref d) = claude_dir {
        collect_skills_in(&d.join("skills"), SkillSource::User, &mut active_skills);
        // Plugin-bundled skills: only count those whose owning plugin is
        // enabled (avoids reporting marketplaces that the user has installed
        // but disabled).
        collect_plugin_skills(d, &enabled_plugins, &mut active_skills);
    }
    collect_skills_in(
        &cwd.join(".claude").join("skills"),
        SkillSource::Project,
        &mut active_skills,
    );

    let mut claude_md_files = BTreeMap::new();
    if let Some(ref d) = claude_dir {
        if let Some((label, hash)) = read_and_hash(&d.join("CLAUDE.md"), "user:CLAUDE.md") {
            claude_md_files.insert(label, hash);
        }
    }
    collect_claude_md_walking_up(cwd, &mut claude_md_files);

    CapturedEnv {
        active_skills,
        claude_md_files,
        active_hook_events,
        enabled_plugins,
    }
}

/// Extract unknown SessionStart payload keys into a bag, so plan-mode /
/// permission-mode / other future-Claude-Code fields land somewhere
/// forward-compatibly without a schema change.
pub fn payload_extras(payload: &Value) -> BTreeMap<String, Value> {
    const KNOWN_KEYS: &[&str] = &["session_id", "cwd", "transcript_path", "model"];
    let mut out = BTreeMap::new();
    if let Some(obj) = payload.as_object() {
        for (k, v) in obj {
            if !KNOWN_KEYS.contains(&k.as_str()) {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    out
}

// ---------- settings.json ----------

fn read_settings_json(path: &Path) -> Option<Value> {
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn count_non_ours_hooks(settings: &Value) -> BTreeMap<String, u32> {
    let mut out = BTreeMap::new();
    let Some(hooks) = settings.get("hooks").and_then(|h| h.as_object()) else {
        return out;
    };
    for (event, entries) in hooks {
        let Some(arr) = entries.as_array() else {
            continue;
        };
        let mut count = 0u32;
        for entry in arr {
            // Two valid shapes: flat `{type, command}` (v0.1.x legacy) and
            // wrapped `{hooks: [{type, command}]}` (current). Walk both.
            for cmd in iter_commands_in_entry(entry) {
                let is_ours = OUR_HOOK_COMMAND_PREFIXES.iter().any(|p| cmd.starts_with(p));
                if !is_ours {
                    count += 1;
                }
            }
        }
        if count > 0 {
            out.insert(event.clone(), count);
        }
    }
    out
}

fn iter_commands_in_entry(entry: &Value) -> Vec<String> {
    let mut out = Vec::new();
    // Flat shape: command at the top level.
    if let Some(c) = entry.get("command").and_then(|c| c.as_str()) {
        out.push(c.to_string());
    }
    // Wrapped shape: { hooks: [{ command }] }.
    if let Some(inner) = entry.get("hooks").and_then(|h| h.as_array()) {
        for h in inner {
            if let Some(c) = h.get("command").and_then(|c| c.as_str()) {
                out.push(c.to_string());
            }
        }
    }
    out
}

fn enabled_plugins_from(settings: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let Some(obj) = settings.get("enabledPlugins").and_then(|v| v.as_object()) else {
        return out;
    };
    for (k, v) in obj {
        if v.as_bool().unwrap_or(false) {
            out.push(k.clone());
        }
    }
    out.sort();
    out
}

// ---------- skills ----------

fn collect_skills_in(dir: &Path, source: SkillSource, out: &mut Vec<SkillRef>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_md = path.join("SKILL.md");
        if let Some((name, hash)) = read_skill(&skill_md, &path) {
            out.push(SkillRef {
                name,
                content_hash: hash,
                source: source.clone(),
            });
        }
    }
}

fn read_skill(skill_md: &Path, skill_dir: &Path) -> Option<(String, String)> {
    let content = fs::read_to_string(skill_md).ok()?;
    let name = skill_dir
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())?;
    Some((name, fnv1a_64_hex(content.as_bytes())))
}

fn collect_plugin_skills(claude_dir: &Path, enabled: &[String], out: &mut Vec<SkillRef>) {
    // The installed copy of each plugin lives under
    //   ~/.claude/plugins/cache/<marketplace>/<plugin>/<version>/skills/<skill>/SKILL.md
    // keyed by the same "<plugin>@<marketplace>" identifier that settings.json
    // records in `enabledPlugins`. We walk the cache tree rather than
    // `marketplaces/`, because a marketplace nests its plugins under
    // per-marketplace subdirs (`plugins/`, `external_plugins/`, …) whose layout
    // varies, while the cache path is uniform and enable-key-addressable. Only
    // enabled plugins are walked; disabled ones never become "active skills".
    let cache_dir = claude_dir.join("plugins").join("cache");
    for key in enabled {
        let Some((plugin, marketplace)) = key.split_once('@') else {
            continue;
        };
        let plugin_dir = cache_dir.join(marketplace).join(plugin);
        let Ok(versions) = fs::read_dir(&plugin_dir) else {
            continue;
        };
        // A plugin dir holds one subdir per installed version. An upgrade can
        // leave more than one on disk, so dedupe skills by name across them.
        let mut seen = std::collections::BTreeSet::new();
        for version in versions.flatten() {
            let mut found = Vec::new();
            collect_skills_in(
                &version.path().join("skills"),
                SkillSource::Plugin(plugin.to_string()),
                &mut found,
            );
            for skill in found {
                if seen.insert(skill.name.clone()) {
                    out.push(skill);
                }
            }
        }
    }
}

// ---------- CLAUDE.md walk ----------

fn collect_claude_md_walking_up(cwd: &Path, out: &mut BTreeMap<String, String>) {
    let mut current = Some(cwd.to_path_buf());
    let mut depth = 0;
    while let Some(dir) = current {
        if depth > CLAUDE_MD_WALK_DEPTH {
            break;
        }
        let candidate = dir.join("CLAUDE.md");
        if let Some((label, hash)) = read_and_hash(&candidate, &candidate.to_string_lossy()) {
            out.insert(label, hash);
        }
        let dot_candidate = dir.join(".claude").join("CLAUDE.md");
        if let Some((label, hash)) = read_and_hash(&dot_candidate, &dot_candidate.to_string_lossy())
        {
            out.insert(label, hash);
        }
        // Stop walking once we hit the repo root — CLAUDE.md files above the
        // repo aren't logically part of this project.
        if dir.join(".git").exists() {
            break;
        }
        current = dir.parent().map(|p| p.to_path_buf());
        depth += 1;
    }
}

fn read_and_hash(path: &Path, label: &str) -> Option<(String, String)> {
    let content = fs::read_to_string(path).ok()?;
    Some((label.to_string(), fnv1a_64_hex(content.as_bytes())))
}

// ---------- hashing ----------

/// FNV-1a 64-bit hash, hex-encoded. 16-char output.
///
/// Not cryptographic — these are grouping keys for the correlation layer.
/// "did the file change?" is the only question we need to answer. FNV-1a
/// avoids pulling in the `sha2` crate.
fn fnv1a_64_hex(bytes: &[u8]) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(PRIME);
    }
    format!("{h:016x}")
}

// Helper for hooks.rs — apply the captured env to a record. Kept here so
// the field-set lives in one place.
pub fn apply(record: &mut crate::model::SessionRecord, env: CapturedEnv) {
    record.active_skills = env.active_skills;
    record.claude_md_files = env.claude_md_files;
    record.active_hook_events = env.active_hook_events;
    record.enabled_plugins = env.enabled_plugins;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn write(dir: &Path, rel: &str, content: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        let mut f = fs::File::create(&p).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn fnv1a_is_deterministic_and_distinguishes_changes() {
        let a = fnv1a_64_hex(b"hello world");
        let b = fnv1a_64_hex(b"hello world");
        let c = fnv1a_64_hex(b"hello world!");
        assert_eq!(a, b, "deterministic");
        assert_ne!(a, c, "single-byte change visible");
        assert_eq!(a.len(), 16, "hex output is 16 chars (64 bits)");
    }

    #[test]
    fn fnv1a_empty_input_is_stable() {
        // Sanity: empty input must hash to the FNV-1a offset basis.
        let h = fnv1a_64_hex(&[]);
        assert_eq!(h, "cbf29ce484222325");
    }

    #[test]
    fn count_non_ours_hooks_excludes_payoff_and_legacy_claude_time() {
        // Wrapped shape with payoff (current) hook, claude-time (legacy)
        // hook, and a real user hook on the same event. Only the user hook
        // counts; both ours and our legacy-name predecessor are excluded.
        let settings = serde_json::json!({
            "hooks": {
                "SessionStart": [
                    { "hooks": [{ "type": "command", "command": "payoff hook session-start" }] },
                    { "hooks": [{ "type": "command", "command": "claude-time hook session-start" }] },
                    { "hooks": [{ "type": "command", "command": "my-tool" }] }
                ],
                "PostToolUse": [
                    { "matcher": "Edit", "hooks": [{ "type": "command", "command": "format-code" }] }
                ]
            }
        });
        let m = count_non_ours_hooks(&settings);
        assert_eq!(
            m.get("SessionStart").copied(),
            Some(1),
            "only user-tool counts; payoff + claude-time both excluded"
        );
        assert_eq!(m.get("PostToolUse").copied(), Some(1));
        // Event with zero non-ours commands shouldn't appear at all.
        let settings_only_ours = serde_json::json!({
            "hooks": {
                "SessionEnd": [
                    { "hooks": [{ "type": "command", "command": "payoff hook session-end" }] }
                ]
            }
        });
        let m = count_non_ours_hooks(&settings_only_ours);
        assert!(m.is_empty(), "events with only our hooks are dropped");
    }

    #[test]
    fn count_non_ours_hooks_handles_legacy_flat_shape() {
        // v0.1.x users may still have flat entries until reinstall migrates.
        let settings = serde_json::json!({
            "hooks": {
                "SessionStart": [
                    { "type": "command", "command": "claude-time hook session-start" },
                    { "type": "command", "command": "user-flat-tool" }
                ]
            }
        });
        let m = count_non_ours_hooks(&settings);
        assert_eq!(m.get("SessionStart").copied(), Some(1));
    }

    #[test]
    fn enabled_plugins_filters_disabled_ones() {
        let settings = serde_json::json!({
            "enabledPlugins": {
                "alpha@mp1": true,
                "beta@mp1": false,
                "gamma@mp2": true
            }
        });
        let v = enabled_plugins_from(&settings);
        assert_eq!(v, vec!["alpha@mp1".to_string(), "gamma@mp2".to_string()]);
    }

    #[test]
    fn skills_collected_from_user_dir() {
        let tmp = TempDir::new().unwrap();
        let claude = tmp.path().join(".claude");
        write(&claude, "skills/alpha/SKILL.md", "alpha body");
        write(&claude, "skills/beta/SKILL.md", "beta body");
        // A directory without SKILL.md must be ignored.
        fs::create_dir_all(claude.join("skills/no-skill-md")).unwrap();

        let mut out = Vec::new();
        collect_skills_in(&claude.join("skills"), SkillSource::User, &mut out);
        let mut names: Vec<&str> = out.iter().map(|s| s.name.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["alpha", "beta"]);
        for s in &out {
            assert_eq!(s.source, SkillSource::User);
            assert_eq!(s.content_hash.len(), 16);
        }
    }

    #[test]
    fn plugin_skills_only_count_when_enabled() {
        let tmp = TempDir::new().unwrap();
        let claude = tmp.path().to_path_buf();
        // Cache layout: plugins/cache/<marketplace>/<plugin>/<version>/skills/.
        // Disabled plugin → its skill must not appear.
        write(
            &claude,
            "plugins/cache/mp1/disabled-plugin/1.0.0/skills/foo/SKILL.md",
            "x",
        );
        // Enabled plugin → its skill must appear.
        write(
            &claude,
            "plugins/cache/mp1/enabled-plugin/1.0.0/skills/bar/SKILL.md",
            "y",
        );

        let mut out = Vec::new();
        collect_plugin_skills(&claude, &["enabled-plugin@mp1".to_string()], &mut out);
        assert_eq!(out.len(), 1, "got {out:?}");
        assert_eq!(out[0].name, "bar");
        assert_eq!(
            out[0].source,
            SkillSource::Plugin("enabled-plugin".to_string())
        );
    }

    #[test]
    fn plugin_skills_deduped_across_leftover_versions() {
        // An upgrade can leave two version dirs on disk. The same skill name
        // present in both must be reported once, not twice.
        let tmp = TempDir::new().unwrap();
        let claude = tmp.path().to_path_buf();
        write(
            &claude,
            "plugins/cache/mp1/p/1.0.0/skills/shared/SKILL.md",
            "old",
        );
        write(
            &claude,
            "plugins/cache/mp1/p/2.0.0/skills/shared/SKILL.md",
            "new",
        );
        write(
            &claude,
            "plugins/cache/mp1/p/2.0.0/skills/fresh/SKILL.md",
            "only-in-new",
        );

        let mut out = Vec::new();
        collect_plugin_skills(&claude, &["p@mp1".to_string()], &mut out);
        let mut names: Vec<&str> = out.iter().map(|s| s.name.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["fresh", "shared"], "got {out:?}");
    }

    #[test]
    fn claude_md_walks_up_to_repo_root_then_stops() {
        let tmp = TempDir::new().unwrap();
        // Layout:
        //   tmp/CLAUDE.md         (above the repo — must NOT be captured)
        //   tmp/repo/.git/
        //   tmp/repo/CLAUDE.md    (project root)
        //   tmp/repo/sub/CLAUDE.md (deeper level — also captured)
        //   tmp/repo/sub/.claude/CLAUDE.md  (dotted form — also captured)
        let above = tmp.path().to_path_buf();
        write(&above, "CLAUDE.md", "above repo, should be ignored");
        let repo = above.join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        write(&repo, "CLAUDE.md", "root CLAUDE");
        write(&repo, "sub/CLAUDE.md", "sub CLAUDE");
        write(&repo, "sub/.claude/CLAUDE.md", "dotted CLAUDE");

        let mut out = BTreeMap::new();
        collect_claude_md_walking_up(&repo.join("sub"), &mut out);

        // The walk should pick up: sub/CLAUDE.md, sub/.claude/CLAUDE.md, repo/CLAUDE.md.
        // The above-repo CLAUDE.md must NOT be captured (.git stops the walk).
        let labels: Vec<String> = out.keys().cloned().collect();
        let outside_label = above.join("CLAUDE.md").to_string_lossy().to_string();
        assert!(
            !labels.iter().any(|l| l == &outside_label),
            "outside-repo CLAUDE.md leaked: {labels:?}"
        );
        // Three real captures expected.
        assert_eq!(
            out.len(),
            3,
            "expected sub/, sub/.claude/, repo/ CLAUDE.md — got {labels:?}"
        );
    }

    #[test]
    fn payload_extras_keeps_only_unknown_keys() {
        let payload = serde_json::json!({
            "session_id": "x",
            "cwd": "/tmp",
            "transcript_path": "/t",
            "model": "opus",
            "plan_mode": true,
            "permission_mode": "default"
        });
        let extras = payload_extras(&payload);
        assert_eq!(extras.len(), 2);
        assert_eq!(extras.get("plan_mode"), Some(&serde_json::json!(true)));
        assert_eq!(
            extras.get("permission_mode"),
            Some(&serde_json::json!("default"))
        );
    }

    #[test]
    fn capture_against_empty_dirs_returns_empty_env() {
        // Hook is invoked from an arbitrary directory with no .claude/, no
        // settings.json, no CLAUDE.md. Must not error, must not panic.
        let tmp = TempDir::new().unwrap();
        let env = capture_with_claude_dir(tmp.path(), Some(tmp.path().join("non-existent")));
        assert!(env.active_skills.is_empty());
        assert!(env.claude_md_files.is_empty());
        assert!(env.active_hook_events.is_empty());
        assert!(env.enabled_plugins.is_empty());
    }

    #[test]
    fn capture_end_to_end_against_synthetic_setup() {
        // Build a full ~/.claude with skills + settings + plugin marketplace,
        // plus a project cwd with CLAUDE.md + project skill, and assert all
        // four CapturedEnv fields populate as expected.
        let tmp = TempDir::new().unwrap();
        let claude = tmp.path().join("dot-claude");
        write(&claude, "skills/refactor/SKILL.md", "refactor body");
        write(
            &claude,
            "settings.json",
            r#"{
            "hooks": {
                "SessionStart": [
                    { "hooks": [{ "type": "command", "command": "payoff hook session-start" }] },
                    { "hooks": [{ "type": "command", "command": "my-tool" }] }
                ]
            },
            "enabledPlugins": { "alpha@mp1": true, "beta@mp1": false }
        }"#,
        );
        // Plugin cache skill: alpha is enabled, beta isn't.
        write(
            &claude,
            "plugins/cache/mp1/alpha/1.0.0/skills/alpha-skill/SKILL.md",
            "alpha body",
        );
        write(
            &claude,
            "plugins/cache/mp1/beta/1.0.0/skills/beta-skill/SKILL.md",
            "beta body",
        );
        write(&claude, "CLAUDE.md", "user-global CLAUDE.md");

        // Project layout with a CLAUDE.md and a project-level skill.
        let repo = tmp.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        write(&repo, "CLAUDE.md", "project CLAUDE.md");
        write(&repo, ".claude/skills/proj-skill/SKILL.md", "proj skill");

        let env = capture_with_claude_dir(&repo, Some(claude.clone()));

        // Skills: refactor (user) + alpha-skill (plugin, enabled) + proj-skill (project).
        // beta-skill is filtered out because beta@mp1 is disabled.
        let names: Vec<String> = env.active_skills.iter().map(|s| s.name.clone()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(
            sorted,
            vec![
                "alpha-skill".to_string(),
                "proj-skill".to_string(),
                "refactor".to_string()
            ],
            "got {names:?}"
        );

        // Hooks: SessionStart=1 (our hook excluded, user-tool counted).
        assert_eq!(env.active_hook_events.get("SessionStart").copied(), Some(1));

        // Enabled plugins: only alpha.
        assert_eq!(env.enabled_plugins, vec!["alpha@mp1".to_string()]);

        // CLAUDE.md: user-global + project root. (We're invoked from repo
        // root itself so there's no deeper walk to do.)
        assert_eq!(
            env.claude_md_files.len(),
            2,
            "got {:?}",
            env.claude_md_files
        );
    }
}
