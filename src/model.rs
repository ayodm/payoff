//! Per-session record and quadrant scoring.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
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

    // Populated from git diff at SessionEnd.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub lines_added: u32,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub lines_removed: u32,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub files_changed: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_diffs: BTreeMap<String, FileDiff>,
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
}
