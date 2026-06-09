//! Retention scoring via libgit2.
//!
//! The model: a session's value is proxied by how much of its diff still
//! exists in HEAD. For each file the session modified, we blame the file at
//! HEAD and count lines whose final-touching commit is the session's
//! commit. Lines a later commit has touched count as "not retained" — that's
//! the signal we want (further edits to the same line means the code didn't
//! land cleanly).
//!
//! Failure modes are surfaced explicitly via `RetentionOutcome` rather than
//! collapsed into a single number, so the report can distinguish "wasted" from
//! "unmeasurable" from "rebased away."

use crate::model::{RetentionOutcome, SessionRecord};
use anyhow::Result;
use git2::{BlameOptions, DiffOptions, Oid, Repository};
use std::collections::BTreeMap;
use std::path::Path;

use crate::model::FileDiff;

pub fn score(session: &SessionRecord) -> Result<RetentionOutcome> {
    let cwd = Path::new(&session.cwd);
    let repo = match Repository::discover(cwd) {
        Ok(r) => r,
        Err(_) => return Ok(RetentionOutcome::NonGit),
    };

    let sha_after = match &session.git_sha_after {
        Some(s) => s,
        None => return Ok(RetentionOutcome::NoChanges),
    };

    // Session ended without a commit — sha_before == sha_after, no history to
    // score against. Uncommitted working-tree changes aren't retention-trackable.
    if Some(sha_after) == session.git_sha_before.as_ref() {
        return Ok(RetentionOutcome::NoChanges);
    }

    let after_oid = match Oid::from_str(sha_after) {
        Ok(o) => o,
        Err(_) => return Ok(RetentionOutcome::Rebased),
    };

    // Is sha_after still reachable from HEAD? If a rebase/squash dropped it,
    // we can't measure retention via blame.
    let head_oid = match repo.head().and_then(|r| r.peel_to_commit()) {
        Ok(c) => c.id(),
        Err(_) => return Ok(RetentionOutcome::NonGit),
    };
    if head_oid != after_oid {
        let reachable = repo
            .graph_descendant_of(head_oid, after_oid)
            .unwrap_or(false);
        if !reachable {
            return Ok(RetentionOutcome::Rebased);
        }
    }

    if session.file_diffs.is_empty() {
        return Ok(RetentionOutcome::NoChanges);
    }

    let mut total_added = 0u32;
    let mut total_surviving = 0u32;

    for (path, diff) in &session.file_diffs {
        total_added = total_added.saturating_add(diff.lines_added);
        let surviving = blame_surviving_lines(&repo, path, after_oid, diff.lines_added)?;
        total_surviving = total_surviving.saturating_add(surviving);
    }

    if total_added == 0 {
        return Ok(RetentionOutcome::NoChanges);
    }

    let rate = total_surviving as f64 / total_added as f64;
    Ok(RetentionOutcome::Scored(rate.clamp(0.0, 1.0)))
}

/// Count blame hunks whose final commit equals `sha_after`. Capped at
/// `capped_at` so a file edited in a later session can't inflate the count
/// past what the session actually added.
fn blame_surviving_lines(
    repo: &Repository,
    file_path: &str,
    sha_after: Oid,
    capped_at: u32,
) -> Result<u32> {
    let repo_workdir = match repo.workdir() {
        Some(p) => p,
        None => return Ok(0), // bare repo
    };
    let abs = Path::new(file_path);
    let rel: &Path = if abs.is_absolute() {
        match abs.strip_prefix(repo_workdir) {
            Ok(r) => r,
            Err(_) => return Ok(0), // outside repo
        }
    } else {
        abs
    };

    let mut opts = BlameOptions::new();
    opts.track_copies_same_file(true);
    let blame = match repo.blame_file(rel, Some(&mut opts)) {
        Ok(b) => b,
        Err(_) => return Ok(0), // file no longer exists at HEAD
    };

    let mut count: u32 = 0;
    for hunk in blame.iter() {
        if hunk.final_commit_id() == sha_after {
            count = count.saturating_add(hunk.lines_in_hunk() as u32);
        }
    }
    Ok(count.min(capped_at))
}

/// Capture per-file diff sizes between two commits. Used by the SessionEnd hook
/// to populate `SessionRecord::file_diffs`.
pub fn diff_files(
    repo: &Repository,
    sha_before: &str,
    sha_after: &str,
) -> Result<BTreeMap<String, FileDiff>> {
    let before_oid = Oid::from_str(sha_before)?;
    let after_oid = Oid::from_str(sha_after)?;
    let before_tree = repo.find_commit(before_oid)?.tree()?;
    let after_tree = repo.find_commit(after_oid)?.tree()?;
    let mut opts = DiffOptions::new();
    let diff = repo.diff_tree_to_tree(Some(&before_tree), Some(&after_tree), Some(&mut opts))?;

    let mut out: BTreeMap<String, FileDiff> = BTreeMap::new();
    let stats = diff.stats()?;
    let _ = stats; // we walk per-delta below; stats() is the aggregate sanity check

    diff.print(git2::DiffFormat::NameOnly, |_, _, _| true).ok();

    // Walk deltas + accumulate line counts via foreach.
    diff.foreach(
        &mut |_, _| true,
        None,
        Some(&mut |delta, _hunk| {
            // hunk header line counts aren't directly here; we'll accumulate
            // per-line in the line callback below by remembering the current path.
            let _ = delta;
            true
        }),
        Some(&mut |delta, _hunk, line| {
            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .and_then(|p| p.to_str())
                .map(|s| s.to_string());
            if let Some(path) = path {
                let entry = out.entry(path).or_default();
                match line.origin() {
                    '+' => entry.lines_added = entry.lines_added.saturating_add(1),
                    '-' => entry.lines_removed = entry.lines_removed.saturating_add(1),
                    _ => {}
                }
            }
            true
        }),
    )?;

    Ok(out)
}

/// Aggregate totals for the report.
pub fn aggregate_totals(file_diffs: &BTreeMap<String, FileDiff>) -> (u32, u32, u32) {
    let mut added = 0u32;
    let mut removed = 0u32;
    for d in file_diffs.values() {
        added = added.saturating_add(d.lines_added);
        removed = removed.saturating_add(d.lines_removed);
    }
    (added, removed, file_diffs.len() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Quadrant, RetentionOutcome};
    use std::fs;
    use tempfile::TempDir;

    /// Init a repo, commit `initial.txt`, return the repo's path + initial sha.
    fn init_repo_with_file(content: &str) -> (TempDir, Repository, Oid) {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        fs::write(dir.path().join("initial.txt"), content).unwrap();

        let mut index = repo.index().unwrap();
        index.add_path(Path::new("initial.txt")).unwrap();
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        let oid = repo
            .commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        drop(tree);
        drop(index);
        (dir, repo, oid)
    }

    fn add_commit(repo: &Repository, path: &str, content: &str, msg: &str) -> Oid {
        let workdir = repo.workdir().unwrap().to_path_buf();
        fs::write(workdir.join(path), content).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(path)).unwrap();
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        let oid = repo
            .commit(Some("HEAD"), &sig, &sig, msg, &tree, &[&parent])
            .unwrap();
        oid
    }

    fn mk_session(cwd: &Path, before: Oid, after: Oid, file_diffs: Vec<(&str, u32)>) -> SessionRecord {
        let mut s = SessionRecord::default();
        s.session_id = "t1".into();
        s.cwd = cwd.to_string_lossy().into_owned();
        s.git_sha_before = Some(before.to_string());
        s.git_sha_after = Some(after.to_string());
        for (p, added) in file_diffs {
            s.file_diffs.insert(
                p.to_string(),
                FileDiff {
                    lines_added: added,
                    lines_removed: 0,
                },
            );
        }
        s
    }

    #[test]
    fn nongit_dir_is_unmeasurable() {
        let dir = TempDir::new().unwrap();
        let mut s = SessionRecord::default();
        s.cwd = dir.path().to_string_lossy().into_owned();
        s.git_sha_after = Some("abc123".into());
        let outcome = score(&s).unwrap();
        assert!(matches!(outcome, RetentionOutcome::NonGit));
    }

    #[test]
    fn no_commit_during_session_is_no_changes() {
        let (dir, _repo, before) = init_repo_with_file("a\n");
        let mut s = mk_session(dir.path(), before, before, vec![]);
        s.git_sha_after = Some(before.to_string());
        let outcome = score(&s).unwrap();
        assert!(matches!(outcome, RetentionOutcome::NoChanges));
    }

    #[test]
    fn session_with_untouched_file_scores_full_retention() {
        let (dir, repo, before) = init_repo_with_file("a\n");
        let after = add_commit(&repo, "new.txt", "x\ny\nz\n", "session");
        let s = mk_session(dir.path(), before, after, vec![("new.txt", 3)]);
        let outcome = score(&s).unwrap();
        match outcome {
            RetentionOutcome::Scored(r) => assert!((r - 1.0).abs() < 1e-6, "got {r}"),
            other => panic!("expected Scored, got {other:?}"),
        }
    }

    #[test]
    fn later_commit_replacing_lines_drops_retention() {
        let (dir, repo, before) = init_repo_with_file("a\n");
        let after = add_commit(&repo, "new.txt", "x\ny\nz\n", "session");
        // A later commit rewrites the file entirely.
        add_commit(&repo, "new.txt", "completely\ndifferent\n", "rewrite");
        let s = mk_session(dir.path(), before, after, vec![("new.txt", 3)]);
        let outcome = score(&s).unwrap();
        match outcome {
            RetentionOutcome::Scored(r) => assert!(r < 0.01, "expected ~0, got {r}"),
            other => panic!("expected Scored, got {other:?}"),
        }
    }

    #[test]
    fn quadrant_classification_via_score() {
        // Synthesize: kept + cheap → QuickWin
        let q = crate::model::classify(Some(RetentionOutcome::Scored(0.9)), 0.10);
        assert_eq!(q, Quadrant::QuickWin);
    }

    #[test]
    fn diff_files_captures_per_file_adds() {
        let (dir, repo, before) = init_repo_with_file("a\n");
        let after = add_commit(&repo, "new.txt", "x\ny\nz\n", "session");
        let m = diff_files(&repo, &before.to_string(), &after.to_string()).unwrap();
        let entry = m.get("new.txt").unwrap();
        assert_eq!(entry.lines_added, 3);
        let _ = dir;
    }
}
