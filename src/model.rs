//! Per-session record and quadrant scoring.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// One Claude Code session captured by claude-time.
///
/// Written initially by the `SessionStart` hook and topped up by `SessionEnd`.
/// Idempotent: re-running either hook with the same `session_id` updates the
/// existing record rather than creating a duplicate.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionRecord {
    pub session_id: String,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_sha_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_sha_after: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,

    // Populated from transcript parse at SessionEnd.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub turn_count: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tool_calls: BTreeMap<String, u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_modified: Vec<String>,
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub total_cost_usd: f64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub input_tokens: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub cache_read_tokens: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub cache_creation_tokens: u64,

    /// Per-file edit count from the transcript (Edit/Write/MultiEdit/NotebookEdit).
    /// Combined with per-file retention this is the pinpoint-waste signal.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub per_file_edits: BTreeMap<String, u32>,

    /// Ordered list of tool-call names as they appeared in the transcript.
    /// Capped at 200 entries to bound storage for long sessions. The
    /// correlation layer mines this for patterns like
    /// "Edit-without-prior-Read" and `Bash(grep) → Read → Edit` 3-grams.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_sequence: Vec<String>,

    // Populated from git diff at SessionEnd.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub lines_added: u32,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub lines_removed: u32,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub files_changed: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_diffs: BTreeMap<String, FileDiff>,

    // -- v0.2.0 driver fields. Captured at SessionStart by env_capture.rs.
    //    Defaults keep v0.1.x records parseable; the correlation layer in
    //    Phase 2 filters groups with n < 3 so empty fields don't pollute.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_skills: Vec<SkillRef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub claude_md_files: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub active_hook_events: BTreeMap<String, u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enabled_plugins: Vec<String>,
    /// Unknown SessionStart payload keys (e.g. future plan_mode /
    /// permission_mode fields). Captured forward-compatibly without a
    /// schema change required to surface them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env_extras: BTreeMap<String, Value>,
}

/// Identity + content-hash of one skill that was available at SessionStart.
/// Content hash lets the correlation layer detect rule edits across sessions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillRef {
    pub name: String,
    pub content_hash: String,
    pub source: SkillSource,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    User,
    Project,
    Plugin(String),
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}
fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}
fn is_zero_f64(v: &f64) -> bool {
    *v == 0.0
}

/// Per-file diff size at session end (from `git diff sha_before..sha_after`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileDiff {
    pub lines_added: u32,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub lines_removed: u32,
}

/// A concrete waste-pinpoint surface: one file in one session that absorbed
/// edits but whose changes didn't survive. The report ranks these so the
/// operator sees *what* went wrong, not just *that* something did.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WastePinpoint {
    pub session_id: String,
    pub project: Option<String>,
    pub file: String,
    pub edits: u32,
    pub lines_added: u32,
    pub retention: f64,
    pub severity: PinpointSeverity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PinpointSeverity {
    /// 5+ edits, retention near zero — operator should look here first.
    Severe,
    /// 3+ edits with <50% retention — visible churn that didn't stick.
    Iterated,
    /// 1-2 edits and total retention loss — small surface, full loss.
    Lost,
}

impl PinpointSeverity {
    pub fn label(self) -> &'static str {
        match self {
            PinpointSeverity::Severe => "SEVERE",
            PinpointSeverity::Iterated => "ITERATED",
            PinpointSeverity::Lost => "LOST",
        }
    }
}

impl WastePinpoint {
    pub fn classify(
        session_id: &str,
        project: Option<&str>,
        file: &str,
        edits: u32,
        lines_added: u32,
        retention: f64,
    ) -> Option<Self> {
        let severity = if edits >= 5 && retention < 0.10 {
            PinpointSeverity::Severe
        } else if edits >= 3 && retention < 0.50 {
            PinpointSeverity::Iterated
        } else if edits >= 1 && lines_added > 0 && retention < 0.10 {
            PinpointSeverity::Lost
        } else {
            return None;
        };
        Some(Self {
            session_id: session_id.to_string(),
            project: project.map(|s| s.to_string()),
            file: file.to_string(),
            edits,
            lines_added,
            retention,
            severity,
        })
    }

    /// Higher = worse. Used to rank pinpoints across a window.
    pub fn waste_score(&self) -> f64 {
        let base = (1.0 - self.retention) * (self.edits.max(1) as f64);
        match self.severity {
            PinpointSeverity::Severe => base * 2.0,
            PinpointSeverity::Iterated => base * 1.5,
            PinpointSeverity::Lost => base,
        }
    }
}

impl SessionRecord {
    pub fn duration_minutes(&self) -> Option<f64> {
        match (self.started_at, self.ended_at) {
            (Some(a), Some(b)) => Some((b - a).num_seconds() as f64 / 60.0),
            _ => None,
        }
    }

    /// Cost = Claude $ cost + (duration × hourly_rate / 60).
    pub fn total_cost(&self, hourly_rate_usd: f64) -> f64 {
        let time_cost = self
            .duration_minutes()
            .map(|m| m * (hourly_rate_usd / 60.0))
            .unwrap_or(0.0);
        self.total_cost_usd + time_cost
    }

    /// Fraction of input tokens served from cache. `None` when nothing was
    /// processed (no tokens of any kind), so callers can distinguish "session
    /// had no LLM activity" from "session ran cold."
    pub fn cache_hit_ratio(&self) -> Option<f64> {
        let denom = self.cache_read_tokens + self.cache_creation_tokens + self.input_tokens;
        if denom == 0 {
            return None;
        }
        Some(self.cache_read_tokens as f64 / denom as f64)
    }
}

/// Outcome classification given cost + retention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quadrant {
    QuickWin,
    DeepValue,
    CheapWaste,
    ExpensiveWaste,
    /// No git context — retention is unmeasurable.
    Unmeasurable,
    /// Too soon since the session for retention to be meaningful.
    Pending,
    /// Session crossed a rebase / squash — retention signal lost.
    Rebased,
}

impl Quadrant {
    pub fn label(self) -> &'static str {
        match self {
            Quadrant::QuickWin => "QUICK WIN",
            Quadrant::DeepValue => "DEEP VALUE",
            Quadrant::CheapWaste => "CHEAP WASTE",
            Quadrant::ExpensiveWaste => "EXPENSIVE WASTE",
            Quadrant::Unmeasurable => "UNMEASURABLE",
            Quadrant::Pending => "PENDING",
            Quadrant::Rebased => "REBASED",
        }
    }
}

/// Retention threshold above which a session counts as "kept."
pub const RETENTION_HIGH: f64 = 0.5;
/// Cost threshold (dollars) above which a session counts as "high cost."
pub const COST_HIGH_USD: f64 = 1.0;

/// Classify a session given its retention rate and total cost.
pub fn classify(retention: Option<RetentionOutcome>, cost_usd: f64) -> Quadrant {
    let retention = match retention {
        Some(r) => r,
        None => return Quadrant::Pending,
    };
    let rate = match retention {
        RetentionOutcome::Scored(r) => r,
        RetentionOutcome::NonGit => return Quadrant::Unmeasurable,
        RetentionOutcome::Rebased => return Quadrant::Rebased,
        RetentionOutcome::NoChanges => return Quadrant::Unmeasurable,
    };
    let kept = rate >= RETENTION_HIGH;
    let pricey = cost_usd >= COST_HIGH_USD;
    match (kept, pricey) {
        (true, false) => Quadrant::QuickWin,
        (true, true) => Quadrant::DeepValue,
        (false, false) => Quadrant::CheapWaste,
        (false, true) => Quadrant::ExpensiveWaste,
    }
}

/// Result of attempting to score a session's retention.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RetentionOutcome {
    /// Retention rate in [0.0, 1.0].
    Scored(f64),
    /// Session ran outside a git repo.
    NonGit,
    /// Git history changed shape (rebase/squash) — signal lost.
    Rebased,
    /// Session had no file changes to score.
    NoChanges,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_pending_when_no_retention() {
        assert_eq!(classify(None, 0.5), Quadrant::Pending);
    }

    #[test]
    fn classify_quick_win() {
        let r = Some(RetentionOutcome::Scored(0.9));
        assert_eq!(classify(r, 0.10), Quadrant::QuickWin);
    }

    #[test]
    fn classify_deep_value() {
        let r = Some(RetentionOutcome::Scored(0.9));
        assert_eq!(classify(r, 5.0), Quadrant::DeepValue);
    }

    #[test]
    fn classify_cheap_waste() {
        let r = Some(RetentionOutcome::Scored(0.0));
        assert_eq!(classify(r, 0.10), Quadrant::CheapWaste);
    }

    #[test]
    fn classify_expensive_waste() {
        let r = Some(RetentionOutcome::Scored(0.1));
        assert_eq!(classify(r, 5.0), Quadrant::ExpensiveWaste);
    }

    #[test]
    fn classify_unmeasurable_non_git() {
        assert_eq!(
            classify(Some(RetentionOutcome::NonGit), 0.5),
            Quadrant::Unmeasurable
        );
    }

    #[test]
    fn cache_hit_ratio_none_when_no_tokens() {
        let s = SessionRecord::default();
        assert_eq!(s.cache_hit_ratio(), None);
    }

    #[test]
    fn cache_hit_ratio_all_cache_is_one() {
        let mut s = SessionRecord::default();
        s.cache_read_tokens = 1000;
        // No input or cache_creation tokens — fully warm.
        assert_eq!(s.cache_hit_ratio(), Some(1.0));
    }

    #[test]
    fn cache_hit_ratio_no_cache_is_zero() {
        let mut s = SessionRecord::default();
        s.input_tokens = 500;
        assert_eq!(s.cache_hit_ratio(), Some(0.0));
    }

    #[test]
    fn cache_hit_ratio_mixed_inputs() {
        let mut s = SessionRecord::default();
        s.cache_read_tokens = 750;
        s.cache_creation_tokens = 100;
        s.input_tokens = 150;
        // 750 / (750 + 100 + 150) = 750 / 1000 = 0.75
        let r = s.cache_hit_ratio().unwrap();
        assert!((r - 0.75).abs() < 1e-9, "got {r}");
    }

    #[test]
    fn cache_creation_alone_does_not_count_as_hit() {
        // Cache creation is the cost of warming — it's not a hit.
        let mut s = SessionRecord::default();
        s.cache_creation_tokens = 1000;
        assert_eq!(s.cache_hit_ratio(), Some(0.0));
    }

    #[test]
    fn pinpoint_classify_severe() {
        let p = WastePinpoint::classify("s", None, "src/x.rs", 6, 50, 0.05).unwrap();
        assert_eq!(p.severity, PinpointSeverity::Severe);
    }

    #[test]
    fn pinpoint_classify_iterated() {
        let p = WastePinpoint::classify("s", None, "src/x.rs", 4, 30, 0.30).unwrap();
        assert_eq!(p.severity, PinpointSeverity::Iterated);
    }

    #[test]
    fn pinpoint_classify_lost() {
        let p = WastePinpoint::classify("s", None, "src/x.rs", 1, 10, 0.0).unwrap();
        assert_eq!(p.severity, PinpointSeverity::Lost);
    }

    #[test]
    fn pinpoint_none_for_high_retention() {
        // Lots of edits but they all stuck — not waste.
        assert!(WastePinpoint::classify("s", None, "src/x.rs", 10, 100, 0.95).is_none());
    }

    #[test]
    fn pinpoint_severe_outranks_lost() {
        let severe = WastePinpoint::classify("s", None, "x", 6, 50, 0.05).unwrap();
        let lost = WastePinpoint::classify("s", None, "y", 1, 10, 0.0).unwrap();
        assert!(severe.waste_score() > lost.waste_score());
    }
}
