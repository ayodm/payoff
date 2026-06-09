//! Weekly markdown report. Aggregates sessions in a time window, scores each
//! session's retention, classifies into quadrants, renders to stdout.

use crate::config::Config;
use crate::model::{classify, Quadrant, RetentionOutcome, SessionRecord};
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use std::collections::BTreeMap;
use std::fs;

pub fn run(since: &str, retention_window: Option<u32>, by: Option<&str>) -> Result<()> {
    let cfg = crate::config::load()?;
    let window_days = retention_window.unwrap_or(cfg.report.retention_window_days);

    // Opportunistic compaction: roll closed sessions older than the retention
    // window into archive.jsonl so block-overhead doesn't bloat the data dir.
    let archive_cutoff = crate::storage::default_archive_cutoff(window_days);
    let _ = crate::storage::archive_older_than(archive_cutoff);

    let cutoff = parse_since(since)?;
    let mut sessions = load_sessions_since(cutoff)?;
    sessions.extend(crate::storage::load_archive_since(cutoff)?);

    let report = render(&sessions, &cfg, by);
    print!("{report}");
    Ok(())
}

/// Parse a window string like "7d", "30d", "24h" into a UTC cutoff.
fn parse_since(s: &str) -> Result<DateTime<Utc>> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("--since cannot be empty");
    }
    let (num, unit) = s.split_at(s.len() - 1);
    let n: i64 = num
        .parse()
        .map_err(|_| anyhow::anyhow!("could not parse --since={s}"))?;
    let dur = match unit {
        "d" => Duration::days(n),
        "h" => Duration::hours(n),
        "w" => Duration::weeks(n),
        other => anyhow::bail!("unknown --since unit `{other}`; use d / h / w"),
    };
    Ok(Utc::now() - dur)
}

fn load_sessions_since(cutoff: DateTime<Utc>) -> Result<Vec<SessionRecord>> {
    let dir = crate::paths::sessions_dir()?;
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let raw = match fs::read_to_string(&path) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let record: SessionRecord = match serde_json::from_str(&raw) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if let Some(end) = record.ended_at {
            if end >= cutoff {
                out.push(record);
            }
        } else if let Some(start) = record.started_at {
            // In-flight sessions count too if their start is in window.
            if start >= cutoff {
                out.push(record);
            }
        }
    }
    // Newest first.
    out.sort_by_key(|r| std::cmp::Reverse(r.started_at));
    Ok(out)
}

#[derive(Debug)]
struct Scored<'a> {
    session: &'a SessionRecord,
    retention: RetentionOutcome,
    quadrant: Quadrant,
    cost_usd: f64,
}

pub fn render(sessions: &[SessionRecord], cfg: &Config, by: Option<&str>) -> String {
    let mut scored: Vec<Scored> = sessions
        .iter()
        .map(|s| {
            let retention = crate::git_history::score(s).unwrap_or(RetentionOutcome::NoChanges);
            let cost = s.total_cost(cfg.report.hourly_rate_usd);
            let quadrant = classify(Some(retention), cost);
            Scored {
                session: s,
                retention,
                quadrant,
                cost_usd: cost,
            }
        })
        .collect();
    scored.sort_by(|a, b| {
        b.session
            .started_at
            .cmp(&a.session.started_at)
    });

    let mut out = String::new();
    out.push_str("# claude-time report\n\n");
    out.push_str(&format!(
        "Window: {} → now · Sessions: {} · Hourly rate: ${:.2}\n\n",
        oldest_label(&scored),
        scored.len(),
        cfg.report.hourly_rate_usd
    ));

    if scored.is_empty() {
        out.push_str("_No sessions captured in this window._\n");
        return out;
    }

    out.push_str(&render_quadrant_block(&scored));
    out.push('\n');
    out.push_str(&render_totals(&scored));
    out.push('\n');
    out.push_str(&render_top(&scored, "Top value", 3, true));
    out.push('\n');
    out.push_str(&render_top(&scored, "Top waste", 3, false));

    if matches!(by, Some("project")) {
        out.push('\n');
        out.push_str(&render_by_project(&scored));
    }

    out.push('\n');
    out.push_str(FOOTER);
    out
}

fn render_quadrant_block(scored: &[Scored]) -> String {
    let mut counts: BTreeMap<&'static str, u32> = BTreeMap::new();
    for s in scored {
        *counts.entry(s.quadrant.label()).or_default() += 1;
    }
    let q = |k: &str| counts.get(k).copied().unwrap_or(0);
    indoc::formatdoc! {r#"
        ## Quadrant

        ```
                          HIGH retention
                                │
        QUICK WIN   ({qw:>3})    │    DEEP VALUE      ({dv:>3})
                                │
        ────────────────────────┼──────────────────────────
                                │
        CHEAP WASTE ({cw:>3})    │    EXPENSIVE WASTE ({ew:>3})
                                │
                          LOW retention
        ```

        Other: PENDING {pe}, REBASED {re}, UNMEASURABLE {un}
        "#,
        qw = q("QUICK WIN"),
        dv = q("DEEP VALUE"),
        cw = q("CHEAP WASTE"),
        ew = q("EXPENSIVE WASTE"),
        pe = q("PENDING"),
        re = q("REBASED"),
        un = q("UNMEASURABLE"),
    }
}

fn render_totals(scored: &[Scored]) -> String {
    let total_cost: f64 = scored.iter().map(|s| s.cost_usd).sum();
    let total_claude_cost: f64 = scored.iter().map(|s| s.session.total_cost_usd).sum();
    let total_added: u32 = scored.iter().map(|s| s.session.lines_added).sum();
    let total_removed: u32 = scored.iter().map(|s| s.session.lines_removed).sum();
    let total_turns: u32 = scored.iter().map(|s| s.session.turn_count).sum();
    let total_minutes: f64 = scored
        .iter()
        .filter_map(|s| s.session.duration_minutes())
        .sum();
    let surviving: u32 = scored
        .iter()
        .map(|s| match s.retention {
            RetentionOutcome::Scored(r) => (s.session.lines_added as f64 * r) as u32,
            _ => 0,
        })
        .sum();
    let retention_pct = if total_added == 0 {
        0.0
    } else {
        100.0 * surviving as f64 / total_added as f64
    };

    indoc::formatdoc! {r#"
        ## Totals

        | Metric | Value |
        |--------|-------|
        | Total cost | ${total_cost:.2} |
        | Claude $ cost | ${total_claude_cost:.4} |
        | Total duration | {total_minutes:.0} min |
        | Total turns | {total_turns} |
        | Lines added | {total_added} |
        | Lines removed | {total_removed} |
        | Lines surviving in HEAD | {surviving} ({retention_pct:.0}%) |
        "#,
    }
}

fn render_top(scored: &[Scored], heading: &str, n: usize, value: bool) -> String {
    let mut copy: Vec<&Scored> = scored.iter().collect();
    if value {
        // Highest retention × lines added, tie-break on cost asc.
        copy.sort_by(|a, b| {
            let av = value_score(a);
            let bv = value_score(b);
            bv.partial_cmp(&av).unwrap()
        });
    } else {
        // Highest cost with low retention.
        copy.sort_by(|a, b| {
            let av = waste_score(a);
            let bv = waste_score(b);
            bv.partial_cmp(&av).unwrap()
        });
    }
    let mut out = String::new();
    out.push_str(&format!("## {heading}\n\n"));
    if copy.is_empty() {
        out.push_str("_(none)_\n");
        return out;
    }
    out.push_str("| Session | Project | Quadrant | Lines+ | Retention | Cost | Duration |\n");
    out.push_str("|---------|---------|----------|--------|-----------|------|----------|\n");
    for s in copy.iter().take(n) {
        out.push_str(&format_row(s));
    }
    out
}

fn value_score(s: &Scored) -> f64 {
    let r = match s.retention {
        RetentionOutcome::Scored(r) => r,
        _ => 0.0,
    };
    r * s.session.lines_added as f64
}

fn waste_score(s: &Scored) -> f64 {
    let r = match s.retention {
        RetentionOutcome::Scored(r) => r,
        RetentionOutcome::NoChanges => 0.0,
        _ => return -1.0, // unmeasurable / rebased / pending don't count
    };
    (1.0 - r) * s.cost_usd
}

fn format_row(s: &Scored) -> String {
    let id_short = &s.session.session_id[..s.session.session_id.len().min(8)];
    let project = s.session.project.as_deref().unwrap_or("-");
    let retention = match s.retention {
        RetentionOutcome::Scored(r) => format!("{:.0}%", r * 100.0),
        RetentionOutcome::NonGit => "no-git".into(),
        RetentionOutcome::Rebased => "rebased".into(),
        RetentionOutcome::NoChanges => "no-diff".into(),
    };
    let duration = s
        .session
        .duration_minutes()
        .map(|m| format!("{m:.0}m"))
        .unwrap_or_else(|| "-".to_string());
    format!(
        "| `{id_short}` | {project} | {} | {} | {} | ${:.2} | {duration} |\n",
        s.quadrant.label(),
        s.session.lines_added,
        retention,
        s.cost_usd,
    )
}

fn render_by_project(scored: &[Scored]) -> String {
    let mut groups: BTreeMap<String, Vec<&Scored>> = BTreeMap::new();
    for s in scored {
        let key = s.session.project.clone().unwrap_or_else(|| "-".to_string());
        groups.entry(key).or_default().push(s);
    }
    let mut out = String::new();
    out.push_str("## By project\n\n");
    out.push_str("| Project | Sessions | Cost | Lines+ | Surviving |\n");
    out.push_str("|---------|----------|------|--------|-----------|\n");
    for (proj, ss) in groups {
        let cost: f64 = ss.iter().map(|s| s.cost_usd).sum();
        let added: u32 = ss.iter().map(|s| s.session.lines_added).sum();
        let surviving: u32 = ss
            .iter()
            .map(|s| match s.retention {
                RetentionOutcome::Scored(r) => (s.session.lines_added as f64 * r) as u32,
                _ => 0,
            })
            .sum();
        out.push_str(&format!(
            "| {proj} | {} | ${cost:.2} | {added} | {surviving} |\n",
            ss.len()
        ));
    }
    out
}

fn oldest_label(scored: &[Scored]) -> String {
    scored
        .iter()
        .filter_map(|s| s.session.started_at)
        .min()
        .map(|t| t.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "(none)".to_string())
}

const FOOTER: &str = "\
---

_What this report DOES NOT measure: absolute time saved (no baseline), code\
 quality beyond retention, subjective satisfaction, or learning value._\
\n";

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn mk(
        id: &str,
        project: &str,
        added: u32,
        cost: f64,
        retention: f64,
        minutes: f64,
    ) -> SessionRecord {
        let mut s = SessionRecord::default();
        s.session_id = id.to_string();
        s.project = Some(project.to_string());
        s.cwd = "/no/such/path".to_string(); // forces NonGit, but render() uses last_retention
        s.lines_added = added;
        s.total_cost_usd = cost;
        let _ = retention; // placeholder — retention is recomputed at report time
        let start = Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
        s.started_at = Some(start);
        s.ended_at = Some(start + Duration::milliseconds((minutes * 60_000.0) as i64));
        s
    }

    #[test]
    fn parse_since_handles_d_h_w() {
        assert!(parse_since("7d").is_ok());
        assert!(parse_since("24h").is_ok());
        assert!(parse_since("2w").is_ok());
        assert!(parse_since("7x").is_err());
        assert!(parse_since("").is_err());
    }

    #[test]
    fn empty_render_says_no_sessions() {
        let r = render(&[], &Config::default(), None);
        assert!(r.contains("No sessions captured"));
    }

    #[test]
    fn render_includes_quadrant_and_footer() {
        let s = vec![mk("abcd1234", "demo", 100, 0.50, 0.9, 5.0)];
        let out = render(&s, &Config::default(), Some("project"));
        assert!(out.contains("## Quadrant"));
        assert!(out.contains("## Totals"));
        assert!(out.contains("## By project"));
        assert!(out.contains("What this report DOES NOT measure"));
        assert!(out.contains("`abcd1234`"));
    }
}
