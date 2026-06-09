//! Accuracy tests for the retention scorer + pinpoint classifier.
//!
//! These tests exercise behaviors the existing unit tests skip:
//!   - Partial retention (the middle of the 0%-100% range)
//!   - Multi-file weighted aggregation
//!   - Rebased outcome detection
//!   - File-deletion post-session
//!   - Per-file vs aggregate consistency
//!   - WastePinpoint boundary classification
//!   - Pinpoint ranking via waste_score
//!
//! Run with `cargo test --test scoring_accuracy -- --nocapture` to see the
//! computed-vs-expected values inline.

use claude_time::git_history::{score, score_per_file, pinpoint_waste};
use claude_time::model::{
    classify, FileDiff, PinpointSeverity, Quadrant, RetentionOutcome,
    SessionRecord, WastePinpoint, COST_HIGH_USD, RETENTION_HIGH,
};
use git2::{Oid, Repository, Signature};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

// ---------- repo helpers ----------

struct Repo {
    dir: TempDir,
    repo: Repository,
}

impl Repo {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let mut cfg = repo.config().unwrap();
        cfg.set_str("user.email", "t@t").unwrap();
        cfg.set_str("user.name", "t").unwrap();
        Self { dir, repo }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn commit(&self, files: &[(&str, &str)], msg: &str) -> Oid {
        for (name, content) in files {
            fs::write(self.dir.path().join(name), content).unwrap();
        }
        let mut index = self.repo.index().unwrap();
        for (name, _) in files {
            index.add_path(Path::new(name)).unwrap();
        }
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = self.repo.find_tree(tree_oid).unwrap();
        let sig = Signature::now("t", "t@t").unwrap();
        let parents = match self.repo.head().and_then(|r| r.peel_to_commit()) {
            Ok(c) => vec![c],
            Err(_) => vec![],
        };
        let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
        self.repo
            .commit(Some("HEAD"), &sig, &sig, msg, &tree, &parent_refs)
            .unwrap()
    }

    fn delete_and_commit(&self, name: &str, msg: &str) -> Oid {
        fs::remove_file(self.dir.path().join(name)).unwrap();
        let mut index = self.repo.index().unwrap();
        index.remove_path(Path::new(name)).unwrap();
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = self.repo.find_tree(tree_oid).unwrap();
        let sig = Signature::now("t", "t@t").unwrap();
        let parent = self.repo.head().unwrap().peel_to_commit().unwrap();
        self.repo
            .commit(Some("HEAD"), &sig, &sig, msg, &tree, &[&parent])
            .unwrap()
    }

    /// Reset HEAD to a commit, dropping everything after it (simulates rebase/squash).
    fn reset_hard(&self, oid: Oid) {
        let obj = self.repo.find_object(oid, None).unwrap();
        self.repo
            .reset(&obj, git2::ResetType::Hard, None)
            .unwrap();
    }
}

fn mk_session(repo: &Repo, before: Oid, after: Oid, diffs: &[(&str, u32)]) -> SessionRecord {
    let mut s = SessionRecord {
        session_id: "test-session".into(),
        cwd: repo.path().to_string_lossy().into_owned(),
        git_sha_before: Some(before.to_string()),
        git_sha_after: Some(after.to_string()),
        ..Default::default()
    };
    let mut file_diffs = BTreeMap::new();
    for (name, added) in diffs {
        file_diffs.insert(
            name.to_string(),
            FileDiff {
                lines_added: *added,
                lines_removed: 0,
            },
        );
        s.lines_added = s.lines_added.saturating_add(*added);
    }
    s.file_diffs = file_diffs;
    s
}

fn approx(actual: f64, expected: f64, tol: f64) -> bool {
    (actual - expected).abs() < tol
}

// ---------- retention scoring ----------

#[test]
fn partial_retention_when_some_lines_later_replaced() {
    // Session adds 10 lines. A later commit replaces 3 of them.
    // Expected retention: 7/10 = 70%.
    let repo = Repo::new();
    let before = repo.commit(&[("seed.txt", "x\n")], "init");

    let session_content: String = (1..=10).map(|i| format!("line{i}\n")).collect();
    let after = repo.commit(&[("new.txt", &session_content)], "session");

    // Later commit: replace lines 1, 2, 3 with new text. Lines 4-10 untouched.
    let later: String = (1..=10)
        .map(|i| if i <= 3 { format!("rewritten{i}\n") } else { format!("line{i}\n") })
        .collect();
    repo.commit(&[("new.txt", &later)], "later edit");

    let s = mk_session(&repo, before, after, &[("new.txt", 10)]);
    match score(&s).unwrap() {
        RetentionOutcome::Scored(r) => {
            assert!(approx(r, 0.7, 1e-6), "expected 0.70 (7/10), got {r}");
        }
        other => panic!("expected Scored(0.7), got {other:?}"),
    }
}

#[test]
fn multi_file_aggregate_weights_by_lines_added() {
    // File A: 20 lines, fully retained (100%).
    // File B: 5 lines, fully overwritten (0%).
    // Expected aggregate: 20/(20+5) = 80%.
    let repo = Repo::new();
    let before = repo.commit(&[("seed.txt", "x\n")], "init");

    let file_a: String = (1..=20).map(|i| format!("A{i}\n")).collect();
    let file_b: String = (1..=5).map(|i| format!("B{i}\n")).collect();
    let after = repo.commit(&[("a.txt", &file_a), ("b.txt", &file_b)], "session");

    // Rewrite file B entirely.
    let b_replaced: String = (1..=5).map(|i| format!("Z{i}\n")).collect();
    repo.commit(&[("b.txt", &b_replaced)], "trash B");

    let s = mk_session(&repo, before, after, &[("a.txt", 20), ("b.txt", 5)]);
    match score(&s).unwrap() {
        RetentionOutcome::Scored(r) => {
            assert!(approx(r, 0.80, 1e-6), "expected 0.80 (20/25), got {r}");
        }
        other => panic!("expected Scored(0.80), got {other:?}"),
    }
}

#[test]
fn per_file_breakdown_matches_aggregate_when_recombined() {
    // Property check: the per-file map summed by lines_added should
    // recombine into the aggregate score.
    let repo = Repo::new();
    let before = repo.commit(&[("seed.txt", "x\n")], "init");

    let file_a: String = (1..=10).map(|i| format!("A{i}\n")).collect();
    let file_b: String = (1..=10).map(|i| format!("B{i}\n")).collect();
    let after = repo.commit(&[("a.txt", &file_a), ("b.txt", &file_b)], "session");

    // Rewrite half of file B (5 lines).
    let b_partial: String = (1..=10)
        .map(|i| if i <= 5 { format!("Z{i}\n") } else { format!("B{i}\n") })
        .collect();
    repo.commit(&[("b.txt", &b_partial)], "partial B");

    let s = mk_session(&repo, before, after, &[("a.txt", 10), ("b.txt", 10)]);
    let aggregate = match score(&s).unwrap() {
        RetentionOutcome::Scored(r) => r,
        other => panic!("expected Scored, got {other:?}"),
    };
    let per_file = score_per_file(&s).unwrap();

    // Reconstruct from per-file: total_surviving / total_added
    let total_added: u32 = s.file_diffs.values().map(|d| d.lines_added).sum();
    let total_surviving: f64 = per_file
        .iter()
        .map(|(path, rate)| {
            let added = s.file_diffs.get(path).map(|d| d.lines_added).unwrap_or(0);
            (added as f64) * rate
        })
        .sum();
    let reconstructed = total_surviving / total_added as f64;

    eprintln!("aggregate={aggregate} reconstructed={reconstructed} per_file={per_file:?}");
    assert!(
        approx(aggregate, reconstructed, 1e-6),
        "aggregate {aggregate} != reconstructed {reconstructed}"
    );
    // And sanity: 10 fully kept + 5 of 10 kept = 15/20 = 75%.
    assert!(approx(aggregate, 0.75, 1e-6), "expected 0.75, got {aggregate}");
}

#[test]
fn rebased_outcome_when_sha_after_dropped_from_history() {
    // Session commits, then a hard reset drops the commit (simulating a
    // rebase or squash that removed it from history).
    let repo = Repo::new();
    let before = repo.commit(&[("seed.txt", "x\n")], "init");
    let after = repo.commit(&[("new.txt", "a\nb\nc\n")], "session");
    repo.reset_hard(before); // sha_after is now orphaned

    let s = mk_session(&repo, before, after, &[("new.txt", 3)]);
    let outcome = score(&s).unwrap();
    assert!(
        matches!(outcome, RetentionOutcome::Rebased),
        "expected Rebased, got {outcome:?}"
    );
}

#[test]
fn file_deleted_post_session_scores_zero_for_that_file() {
    // Session adds a file. Later commit deletes it. blame_file fails;
    // the file contributes 0 surviving lines.
    let repo = Repo::new();
    let before = repo.commit(&[("seed.txt", "x\n")], "init");
    let after = repo.commit(&[("doomed.txt", "a\nb\nc\nd\n")], "session");
    repo.delete_and_commit("doomed.txt", "remove");

    let s = mk_session(&repo, before, after, &[("doomed.txt", 4)]);
    match score(&s).unwrap() {
        RetentionOutcome::Scored(r) => {
            assert!(approx(r, 0.0, 1e-6), "deleted file should score 0, got {r}");
        }
        other => panic!("expected Scored(0), got {other:?}"),
    }
}

#[test]
fn nochange_when_file_diffs_empty_even_with_distinct_shas() {
    // sha_before != sha_after, but the session's file_diffs map is empty
    // (e.g., the session committed but didn't track per-file diffs).
    let repo = Repo::new();
    let before = repo.commit(&[("seed.txt", "x\n")], "init");
    let after = repo.commit(&[("other.txt", "y\n")], "unrelated");
    let s = mk_session(&repo, before, after, &[]);
    match score(&s).unwrap() {
        RetentionOutcome::NoChanges => {}
        other => panic!("expected NoChanges, got {other:?}"),
    }
}

#[test]
fn per_file_skips_zero_added_files() {
    // A file with 0 lines_added (e.g., pure removal) should not appear in
    // per-file output, since there's nothing to score.
    let repo = Repo::new();
    let before = repo.commit(&[("seed.txt", "x\ny\nz\n")], "init");
    let after = repo.commit(&[("added.txt", "a\nb\n")], "session");

    let mut s = mk_session(&repo, before, after, &[("added.txt", 2)]);
    // Manually add a zero-added entry to file_diffs.
    s.file_diffs.insert(
        "removed.txt".into(),
        FileDiff {
            lines_added: 0,
            lines_removed: 5,
        },
    );
    let per_file = score_per_file(&s).unwrap();
    assert!(
        !per_file.contains_key("removed.txt"),
        "zero-added file should not be in per-file output: {per_file:?}"
    );
    assert!(per_file.contains_key("added.txt"));
}

// ---------- quadrant boundaries ----------

#[test]
fn quadrant_at_retention_boundary_is_kept() {
    // RETENTION_HIGH = 0.5 — exactly at the boundary should count as kept.
    // Test both sides of every threshold so a future refactor that flips a
    // < to <= is caught.
    assert_eq!(RETENTION_HIGH, 0.5);
    assert_eq!(COST_HIGH_USD, 1.0);

    // At exactly 0.5 retention, $0.50 cost — kept + cheap = QuickWin.
    let q = classify(Some(RetentionOutcome::Scored(0.5)), 0.5);
    assert_eq!(q, Quadrant::QuickWin, "at retention=0.5, cost=0.5 -> QuickWin");

    // At 0.4999 retention, $0.50 cost — lost + cheap = CheapWaste.
    let q = classify(Some(RetentionOutcome::Scored(0.4999)), 0.5);
    assert_eq!(q, Quadrant::CheapWaste);

    // At 0.5 retention, exactly $1.00 cost — kept + high cost = DeepValue.
    let q = classify(Some(RetentionOutcome::Scored(0.5)), 1.0);
    assert_eq!(q, Quadrant::DeepValue);

    // At 0.4 retention, $1.5 cost — lost + high cost = ExpensiveWaste.
    let q = classify(Some(RetentionOutcome::Scored(0.4)), 1.5);
    assert_eq!(q, Quadrant::ExpensiveWaste);
}

#[test]
fn quadrant_non_scored_outcomes() {
    assert_eq!(classify(Some(RetentionOutcome::NonGit), 0.5), Quadrant::Unmeasurable);
    assert_eq!(classify(Some(RetentionOutcome::Rebased), 0.5), Quadrant::Rebased);
    assert_eq!(classify(None, 0.5), Quadrant::Pending);
    // NoChanges falls back to Pending in classify — it's only kept distinct
    // in the retention outcome itself.
    let nc = classify(Some(RetentionOutcome::NoChanges), 0.5);
    eprintln!("NoChanges classified as: {nc:?}");
}

// ---------- waste pinpoint boundaries ----------

#[test]
fn pinpoint_severe_at_exact_threshold() {
    // SEVERE: edits >= 5 AND retention < 0.10.
    // At exactly 5 edits and 0.10 retention → fails the < check, falls
    // through. With 3+ edits and <50% it's ITERATED. With <0.10 and 1+
    // edits and lines_added > 0 it would be LOST — but only if edits < 3.
    // So 5 edits at 0.10 retention → ITERATED.
    let p = WastePinpoint::classify("s", None, "f", 5, 100, 0.10).unwrap();
    assert_eq!(p.severity, PinpointSeverity::Iterated);

    // 5 edits at 0.099 retention → SEVERE.
    let p = WastePinpoint::classify("s", None, "f", 5, 100, 0.099).unwrap();
    assert_eq!(p.severity, PinpointSeverity::Severe);

    // 4 edits at 0.05 retention → ITERATED (3+ edits, <50%).
    let p = WastePinpoint::classify("s", None, "f", 4, 100, 0.05).unwrap();
    assert_eq!(p.severity, PinpointSeverity::Iterated);

    // 1 edit at 0.05 retention with lines_added > 0 → LOST.
    let p = WastePinpoint::classify("s", None, "f", 1, 100, 0.05).unwrap();
    assert_eq!(p.severity, PinpointSeverity::Lost);

    // 1 edit at 0.05 retention with lines_added = 0 → not classified
    // (no signal — nothing was added, can't have "lost" it).
    assert!(WastePinpoint::classify("s", None, "f", 1, 0, 0.05).is_none());
}

#[test]
fn pinpoint_high_retention_means_no_pinpoint() {
    // The whole point of pinpoints is *waste* — a file with 100% retention
    // shouldn't classify regardless of edit count.
    assert!(WastePinpoint::classify("s", None, "f", 10, 100, 1.0).is_none());
    assert!(WastePinpoint::classify("s", None, "f", 5, 100, 0.5).is_none());
}

#[test]
fn pinpoint_waste_score_ranks_severe_above_iterated_above_lost() {
    // Same edits and retention, different severities (which we synthesize
    // by tweaking edits and retention to land in each tier).
    let severe = WastePinpoint::classify("s", None, "f1", 5, 100, 0.05).unwrap();
    let iterated = WastePinpoint::classify("s", None, "f2", 3, 100, 0.40).unwrap();
    let lost = WastePinpoint::classify("s", None, "f3", 1, 100, 0.05).unwrap();

    eprintln!(
        "severe={} iterated={} lost={}",
        severe.waste_score(),
        iterated.waste_score(),
        lost.waste_score()
    );
    assert!(severe.waste_score() > iterated.waste_score());
    assert!(iterated.waste_score() > lost.waste_score());
}

// ---------- pinpoint_waste end-to-end ----------

#[test]
fn pinpoint_waste_uses_per_file_retention_not_aggregate() {
    // Two files with very different retention. Pinpoints should use the
    // per-file rates, not the session-aggregate rate.
    let repo = Repo::new();
    let before = repo.commit(&[("seed.txt", "x\n")], "init");

    let file_a: String = (1..=10).map(|i| format!("A{i}\n")).collect();
    let file_b: String = (1..=10).map(|i| format!("B{i}\n")).collect();
    let after = repo.commit(&[("a.txt", &file_a), ("b.txt", &file_b)], "session");

    // Rewrite B entirely. A untouched.
    let b_dead: String = (1..=10).map(|i| format!("dead{i}\n")).collect();
    repo.commit(&[("b.txt", &b_dead)], "kill B");

    let mut s = mk_session(&repo, before, after, &[("a.txt", 10), ("b.txt", 10)]);
    // Simulate transcript edits: 5 on each file.
    s.per_file_edits.insert("a.txt".into(), 5);
    s.per_file_edits.insert("b.txt".into(), 5);

    let pinpoints = pinpoint_waste(&s).unwrap();
    eprintln!("pinpoints={pinpoints:#?}");

    // a.txt should NOT pinpoint (100% retention).
    assert!(
        !pinpoints.iter().any(|p| p.file == "a.txt"),
        "a.txt has full retention — no pinpoint expected"
    );
    // b.txt SHOULD pinpoint as SEVERE (5 edits, 0% retention).
    let b = pinpoints.iter().find(|p| p.file == "b.txt").expect("b.txt should pinpoint");
    assert_eq!(b.severity, PinpointSeverity::Severe);
    assert!(approx(b.retention, 0.0, 1e-6), "expected 0% retention for b, got {}", b.retention);
}

#[test]
fn pinpoint_waste_sorts_by_waste_score_descending() {
    // Construct a session where multiple files pinpoint and verify the
    // returned list is sorted highest-waste first.
    let repo = Repo::new();
    let before = repo.commit(&[("seed.txt", "x\n")], "init");

    // Three files, each fully rewritten after the session.
    let content_a: String = (1..=10).map(|i| format!("A{i}\n")).collect();
    let content_b: String = (1..=10).map(|i| format!("B{i}\n")).collect();
    let content_c: String = (1..=10).map(|i| format!("C{i}\n")).collect();
    let after = repo.commit(
        &[("a.txt", &content_a), ("b.txt", &content_b), ("c.txt", &content_c)],
        "session",
    );
    let dead: String = (1..=10).map(|i| format!("dead{i}\n")).collect();
    repo.commit(
        &[
            ("a.txt", &dead),
            ("b.txt", &dead),
            ("c.txt", &dead),
        ],
        "kill all",
    );

    let mut s = mk_session(
        &repo,
        before,
        after,
        &[("a.txt", 10), ("b.txt", 10), ("c.txt", 10)],
    );
    // Distinct edit counts → distinct severities & scores.
    s.per_file_edits.insert("a.txt".into(), 10); // SEVERE (high edits)
    s.per_file_edits.insert("b.txt".into(), 5);  // SEVERE (moderate edits)
    s.per_file_edits.insert("c.txt".into(), 1);  // LOST (single edit)

    let pinpoints = pinpoint_waste(&s).unwrap();
    eprintln!("ranked pinpoints: {pinpoints:#?}");
    assert_eq!(pinpoints.len(), 3);
    // Highest waste_score first.
    assert!(pinpoints[0].waste_score() >= pinpoints[1].waste_score());
    assert!(pinpoints[1].waste_score() >= pinpoints[2].waste_score());
    // And the highest-edit, full-loss file should be #1.
    assert_eq!(pinpoints[0].file, "a.txt");
}
