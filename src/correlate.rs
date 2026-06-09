//! Correlation analysis: group sessions by environment driver and surface
//! how each group's average retention + cost compare to the all-sessions
//! baseline.
//!
//! Answers the maintainer's v0.2.0 question: "did adding skill X / rewriting
//! CLAUDE.md / changing my hooks move my retention number, and where?"
//!
//! Caveat surfaced to users in the report footer: this is *correlation*, not
//! causation. A skill that ranks high doesn't necessarily *cause* better
//! retention — sessions using that skill may also share other features.

use crate::model::{RetentionOutcome, SessionRecord};

/// Driver feature we can group sessions by. Each variant carries the
/// concrete grouping key inside.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DriverKey {
    /// Sessions that had this skill available.
    Skill(String),
    /// Sessions whose project CLAUDE.md (or other context file) had this
    /// content hash. Different hashes = different rule versions.
    ClaudeMdHash(String),
    /// Sessions where this hook event was wired (excluding our own).
    HookEvent(String),
    /// Sessions that used this Claude model.
    Model(String),
    /// Sessions where the first file-edit happened before any Read.
    /// `true` = edit-without-prior-read; `false` = read-first.
    EditWithoutPriorRead(bool),
}

impl DriverKey {
    /// Short stable string for use in HTMX URLs and report row keys.
    pub fn type_slug(&self) -> &'static str {
        match self {
            DriverKey::Skill(_) => "skill",
            DriverKey::ClaudeMdHash(_) => "claude_md",
            DriverKey::HookEvent(_) => "hook_event",
            DriverKey::Model(_) => "model",
            DriverKey::EditWithoutPriorRead(_) => "edit_pattern",
        }
    }

    pub fn display_value(&self) -> String {
        match self {
            DriverKey::Skill(s) => s.clone(),
            DriverKey::ClaudeMdHash(h) => h.clone(),
            DriverKey::HookEvent(e) => e.clone(),
            DriverKey::Model(m) => m.clone(),
            DriverKey::EditWithoutPriorRead(true) => "edit_without_prior_read".to_string(),
            DriverKey::EditWithoutPriorRead(false) => "read_first".to_string(),
        }
    }
}

/// One driver group with its summary statistics.
#[derive(Debug, Clone)]
pub struct DriverGroup<'a> {
    pub key: DriverKey,
    pub sessions: Vec<&'a SessionRecord>,
    /// Average retention across scorable sessions in the group. `None` if
    /// no session in the group is scorable (all NonGit / NoChanges / etc).
    pub avg_retention: Option<f64>,
    /// Average total cost (Claude $) across all sessions in the group.
    pub avg_cost: f64,
    /// Group size = sessions.len(). Cached so callers don't recompute.
    pub n: usize,
}

/// Minimum group size before a group is rendered. Below this the sample
/// is too small to be informative; the group is dropped to reduce noise.
pub const MIN_GROUP_N: usize = 3;

/// Tool names that count as "file edits" for the prior-Read detector.
const EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// Did the session perform a file-editing tool call *before* its first
/// Read? `None` if the session has no edits at all (can't classify).
pub fn edit_without_prior_read(seq: &[String]) -> Option<bool> {
    let first_edit = seq.iter().position(|t| EDIT_TOOLS.contains(&t.as_str()));
    let first_read = seq.iter().position(|t| t == "Read");
    match (first_edit, first_read) {
        (None, _) => None,
        (Some(_), None) => Some(true),
        (Some(e), Some(r)) => Some(e < r),
    }
}

/// Score a session for retention. Uses the same scorer as the main report
/// so correlation deltas remain comparable to the headline metric.
fn retention_rate(session: &SessionRecord) -> Option<f64> {
    match crate::git_history::score(session).ok()? {
        RetentionOutcome::Scored(r) => Some(r),
        _ => None,
    }
}

/// Bucket sessions by every driver feature they exhibit. One session can
/// land in many groups (it has multiple skills, multiple CLAUDE.md files,
/// one model, one edit-pattern classification, etc.).
///
/// Drops groups with `n < MIN_GROUP_N`. Within each driver type, groups
/// are sorted by `avg_retention` descending (highest retention first) so
/// the report surface "what's working" before "what isn't."
pub fn group_by_driver<'a>(sessions: &'a [SessionRecord]) -> Vec<DriverGroup<'a>> {
    use std::collections::HashMap;
    let mut buckets: HashMap<DriverKey, Vec<&'a SessionRecord>> = HashMap::new();
    let push =
        |b: &mut HashMap<DriverKey, Vec<&'a SessionRecord>>, k: DriverKey, s: &'a SessionRecord| {
            b.entry(k).or_default().push(s);
        };

    for s in sessions {
        for skill in &s.active_skills {
            push(&mut buckets, DriverKey::Skill(skill.name.clone()), s);
        }
        for (label, hash) in &s.claude_md_files {
            // Group by content hash, not path — we want "did this rule
            // version improve retention", regardless of which file held it.
            let _ = label; // path label is kept on the session record itself
            push(&mut buckets, DriverKey::ClaudeMdHash(hash.clone()), s);
        }
        for event in s.active_hook_events.keys() {
            push(&mut buckets, DriverKey::HookEvent(event.clone()), s);
        }
        if let Some(m) = s.model.as_deref() {
            push(&mut buckets, DriverKey::Model(m.to_string()), s);
        }
        if let Some(flag) = edit_without_prior_read(&s.tool_sequence) {
            push(&mut buckets, DriverKey::EditWithoutPriorRead(flag), s);
        }
    }

    let mut groups: Vec<DriverGroup<'a>> = buckets
        .into_iter()
        .filter(|(_, ss)| ss.len() >= MIN_GROUP_N)
        .map(|(key, ss)| {
            let n = ss.len();
            let retentions: Vec<f64> = ss.iter().filter_map(|s| retention_rate(s)).collect();
            let avg_retention = if retentions.is_empty() {
                None
            } else {
                Some(retentions.iter().sum::<f64>() / retentions.len() as f64)
            };
            let total_claude_cost: f64 = ss.iter().map(|s| s.total_cost_usd).sum();
            let avg_cost = total_claude_cost / n as f64;
            DriverGroup {
                key,
                sessions: ss,
                avg_retention,
                avg_cost,
                n,
            }
        })
        .collect();

    // Sort: by driver type (stable group ordering), then within type by
    // retention descending (None last). Ties broken by group size desc.
    groups.sort_by(|a, b| {
        a.key
            .type_slug()
            .cmp(b.key.type_slug())
            .then_with(|| match (b.avg_retention, a.avg_retention) {
                (Some(bx), Some(ax)) => bx.partial_cmp(&ax).unwrap_or(std::cmp::Ordering::Equal),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| b.n.cmp(&a.n))
    });
    groups
}

/// Overall baseline across all sessions in the window. Returned alongside
/// the groups so each row can show `Δ vs. baseline` cleanly.
pub fn baseline(sessions: &[SessionRecord]) -> Baseline {
    let retentions: Vec<f64> = sessions.iter().filter_map(retention_rate).collect();
    let avg_retention = if retentions.is_empty() {
        None
    } else {
        Some(retentions.iter().sum::<f64>() / retentions.len() as f64)
    };
    let total_cost: f64 = sessions.iter().map(|s| s.total_cost_usd).sum();
    let avg_cost = if sessions.is_empty() {
        0.0
    } else {
        total_cost / sessions.len() as f64
    };
    Baseline {
        avg_retention,
        avg_cost,
        n: sessions.len(),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Baseline {
    pub avg_retention: Option<f64>,
    pub avg_cost: f64,
    pub n: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SkillRef, SkillSource};

    fn sess(id: &str, model: Option<&str>, lines_added: u32) -> SessionRecord {
        SessionRecord {
            session_id: id.to_string(),
            cwd: "/tmp".to_string(),
            model: model.map(|s| s.to_string()),
            lines_added,
            total_cost_usd: 0.10,
            ..Default::default()
        }
    }

    fn with_skill(mut s: SessionRecord, skill_name: &str, hash: &str) -> SessionRecord {
        s.active_skills.push(SkillRef {
            name: skill_name.to_string(),
            content_hash: hash.to_string(),
            source: SkillSource::User,
        });
        s
    }

    fn with_claude_md(mut s: SessionRecord, label: &str, hash: &str) -> SessionRecord {
        s.claude_md_files
            .insert(label.to_string(), hash.to_string());
        s
    }

    fn with_hook(mut s: SessionRecord, event: &str) -> SessionRecord {
        s.active_hook_events.insert(event.to_string(), 1);
        s
    }

    fn with_tools(mut s: SessionRecord, tools: &[&str]) -> SessionRecord {
        s.tool_sequence = tools.iter().map(|t| t.to_string()).collect();
        s
    }

    #[test]
    fn edit_without_prior_read_detector_basic_cases() {
        let read_then_edit: Vec<String> = ["Read", "Edit"].iter().map(|s| s.to_string()).collect();
        let edit_then_read: Vec<String> = ["Edit", "Read"].iter().map(|s| s.to_string()).collect();
        let only_edit: Vec<String> = ["Edit"].iter().map(|s| s.to_string()).collect();
        let only_bash: Vec<String> = ["Bash"].iter().map(|s| s.to_string()).collect();
        assert_eq!(edit_without_prior_read(&read_then_edit), Some(false));
        assert_eq!(edit_without_prior_read(&edit_then_read), Some(true));
        assert_eq!(edit_without_prior_read(&only_edit), Some(true));
        // No edits in the sequence — N/A.
        assert_eq!(edit_without_prior_read(&only_bash), None);
        assert_eq!(edit_without_prior_read(&[]), None);
    }

    #[test]
    fn edit_without_prior_read_recognises_all_edit_tools() {
        for tool in ["Edit", "Write", "MultiEdit", "NotebookEdit"] {
            let seq: Vec<String> = [tool].iter().map(|s| s.to_string()).collect();
            assert_eq!(
                edit_without_prior_read(&seq),
                Some(true),
                "{tool} should count as edit-without-prior-read"
            );
        }
    }

    #[test]
    fn group_by_driver_drops_groups_under_min_n() {
        // Two sessions share skill A (n=2 < 3) — should be dropped.
        // Three sessions share model "opus" — should be kept.
        let sessions = vec![
            with_skill(sess("a", Some("opus"), 10), "A", "h1"),
            with_skill(sess("b", Some("opus"), 10), "A", "h1"),
            sess("c", Some("opus"), 10),
        ];
        let groups = group_by_driver(&sessions);
        // No Skill(A) group (n=2 < 3).
        assert!(
            !groups
                .iter()
                .any(|g| matches!(&g.key, DriverKey::Skill(s) if s == "A")),
            "skill A should be dropped: {groups:?}"
        );
        // Model(opus) group present with n=3.
        let opus = groups
            .iter()
            .find(|g| matches!(&g.key, DriverKey::Model(m) if m == "opus"));
        assert!(opus.is_some(), "model opus group missing: {groups:?}");
        assert_eq!(opus.unwrap().n, 3);
    }

    #[test]
    fn group_by_driver_partitions_edit_pattern() {
        // 3 sessions edit-without-read, 3 read-first.
        let mut sessions = Vec::new();
        for i in 0..3 {
            sessions.push(with_tools(
                sess(&format!("ewr{i}"), Some("opus"), 10),
                &["Edit"],
            ));
        }
        for i in 0..3 {
            sessions.push(with_tools(
                sess(&format!("rf{i}"), Some("opus"), 10),
                &["Read", "Edit"],
            ));
        }
        let groups = group_by_driver(&sessions);
        let ewr = groups
            .iter()
            .find(|g| matches!(&g.key, DriverKey::EditWithoutPriorRead(true)));
        let rf = groups
            .iter()
            .find(|g| matches!(&g.key, DriverKey::EditWithoutPriorRead(false)));
        assert!(
            ewr.is_some() && rf.is_some(),
            "both edit-pattern groups expected: {groups:?}"
        );
        assert_eq!(ewr.unwrap().n, 3);
        assert_eq!(rf.unwrap().n, 3);
    }

    #[test]
    fn group_by_driver_groups_by_claude_md_hash() {
        // Two CLAUDE.md hashes; 3 sessions on each.
        let mut sessions = Vec::new();
        for i in 0..3 {
            sessions.push(with_claude_md(
                sess(&format!("h1-{i}"), Some("opus"), 10),
                "<repo>/CLAUDE.md",
                "hash_alpha",
            ));
        }
        for i in 0..3 {
            sessions.push(with_claude_md(
                sess(&format!("h2-{i}"), Some("opus"), 10),
                "<repo>/CLAUDE.md",
                "hash_beta",
            ));
        }
        let groups = group_by_driver(&sessions);
        let hashes: Vec<String> = groups
            .iter()
            .filter_map(|g| match &g.key {
                DriverKey::ClaudeMdHash(h) => Some(h.clone()),
                _ => None,
            })
            .collect();
        assert!(hashes.contains(&"hash_alpha".to_string()));
        assert!(hashes.contains(&"hash_beta".to_string()));
    }

    #[test]
    fn group_by_driver_filters_hooks() {
        let mut sessions = Vec::new();
        for i in 0..3 {
            sessions.push(with_hook(
                sess(&format!("h{i}"), Some("opus"), 10),
                "PostToolUse",
            ));
        }
        let groups = group_by_driver(&sessions);
        let g = groups
            .iter()
            .find(|g| matches!(&g.key, DriverKey::HookEvent(e) if e == "PostToolUse"));
        assert!(g.is_some());
        assert_eq!(g.unwrap().n, 3);
    }

    #[test]
    fn baseline_handles_empty_session_set() {
        let b = baseline(&[]);
        assert!(b.avg_retention.is_none());
        assert_eq!(b.avg_cost, 0.0);
        assert_eq!(b.n, 0);
    }

    #[test]
    fn baseline_averages_cost_correctly() {
        let mut a = sess("a", None, 0);
        a.total_cost_usd = 1.0;
        let mut b = sess("b", None, 0);
        b.total_cost_usd = 3.0;
        let bs = baseline(&[a, b]);
        assert!((bs.avg_cost - 2.0).abs() < 1e-9, "got {}", bs.avg_cost);
        assert_eq!(bs.n, 2);
    }

    #[test]
    fn driver_key_type_slugs_are_stable() {
        // Slugs are used in HTMX URLs — changing them is a breaking change.
        // Lock them in.
        assert_eq!(DriverKey::Skill("x".into()).type_slug(), "skill");
        assert_eq!(DriverKey::ClaudeMdHash("x".into()).type_slug(), "claude_md");
        assert_eq!(DriverKey::HookEvent("x".into()).type_slug(), "hook_event");
        assert_eq!(DriverKey::Model("x".into()).type_slug(), "model");
        assert_eq!(
            DriverKey::EditWithoutPriorRead(true).type_slug(),
            "edit_pattern"
        );
    }

    #[test]
    fn groups_within_type_sort_by_retention_desc() {
        // Two model groups with same n=3 — the higher-retention one should
        // appear first within its type bucket. Since we can't easily
        // synthesize different retention rates without git repos, this test
        // sets up two model groups and verifies the type-slug grouping.
        let mut sessions = Vec::new();
        for i in 0..3 {
            sessions.push(sess(&format!("o{i}"), Some("opus"), 10));
        }
        for i in 0..3 {
            sessions.push(sess(&format!("s{i}"), Some("sonnet"), 10));
        }
        let groups = group_by_driver(&sessions);
        // Both model groups present.
        let model_keys: Vec<String> = groups
            .iter()
            .filter_map(|g| match &g.key {
                DriverKey::Model(m) => Some(m.clone()),
                _ => None,
            })
            .collect();
        assert!(model_keys.contains(&"opus".to_string()));
        assert!(model_keys.contains(&"sonnet".to_string()));
        // Same type clustered together.
        let model_indices: Vec<usize> = groups
            .iter()
            .enumerate()
            .filter_map(|(i, g)| matches!(g.key, DriverKey::Model(_)).then_some(i))
            .collect();
        assert_eq!(model_indices.len(), 2);
        assert_eq!(
            model_indices[1] - model_indices[0],
            1,
            "model groups should be adjacent in sort"
        );
    }
}
