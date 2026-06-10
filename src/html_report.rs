//! HTML + HTMX report renderer.
//!
//! Self-contained: inline CSS, HTMX from CDN. The page renders fully even
//! when no server is running — HTMX attributes are inert without a server.
//! When `claude-time serve` is running, the HTMX attrs become live and
//! enable click-to-expand sessions, window changes, etc.
//!
//! Content prioritization (top to bottom):
//!   1. Top waste pinpoints — *the* answer to "where am I wasting time"
//!   2. Quadrant overview
//!   3. Top value sessions (so the operator sees what worked too)
//!   4. Per-session table
//!   5. Totals + footer caveats

use crate::config::Config;
use crate::model::{
    classify, PinpointSeverity, Quadrant, RetentionOutcome, SessionRecord, WastePinpoint,
};
use std::collections::BTreeMap;

const HTMX_SRC: &str = "https://unpkg.com/htmx.org@2.0.4";

/// Top-level: render the full HTML document.
pub fn render(sessions: &[SessionRecord], cfg: &Config, by: Option<&str>) -> String {
    let scored: Vec<Scored> = sessions
        .iter()
        .map(|s| {
            let retention = crate::git_history::score(s).unwrap_or(RetentionOutcome::NoChanges);
            let cost = s.total_cost(cfg.report.hourly_rate_usd);
            let quadrant = classify(Some(retention), cost);
            let pinpoints = crate::git_history::pinpoint_waste(s).unwrap_or_default();
            Scored {
                session: s,
                retention,
                quadrant,
                cost_usd: cost,
                pinpoints,
            }
        })
        .collect();

    let mut all_pinpoints: Vec<&WastePinpoint> =
        scored.iter().flat_map(|s| s.pinpoints.iter()).collect();
    all_pinpoints.sort_by(|a, b| b.waste_score().partial_cmp(&a.waste_score()).unwrap());

    let body = format!(
        r#"
<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width,initial-scale=1" />
<title>payoff report</title>
<style>{css}</style>
</head>
<body hx-boost="false">
<header class="topbar">
  <div class="brand">payoff</div>
  <div class="meta">{count} sessions · ${total_cost:.2} · {window}</div>
</header>

<main id="main">
  {pinpoints_section}
  {drivers_section}
  {quadrant_section}
  {top_value_section}
  {sessions_section}
  {totals_section}
  {by_section}
</main>

<footer class="caveats">
  <strong>What this report does NOT measure.</strong>
  Absolute time saved (no baseline). Code quality beyond retention.
  Subjective satisfaction. Learning value — a session that taught you
  something has long-tail value retention can't see.
</footer>

<script src="{htmx_src}" defer></script>
</body>
</html>
"#,
        css = STYLE,
        htmx_src = HTMX_SRC,
        count = scored.len(),
        total_cost = scored.iter().map(|s| s.cost_usd).sum::<f64>(),
        window = oldest_label(&scored),
        pinpoints_section = render_pinpoints(&all_pinpoints),
        drivers_section = render_drivers(sessions),
        quadrant_section = render_quadrant(&scored),
        top_value_section = render_top_value(&scored),
        sessions_section = render_sessions(&scored),
        totals_section = render_totals(&scored),
        by_section = if matches!(by, Some("project")) {
            render_by_project(&scored)
        } else {
            String::new()
        },
    );
    body
}

#[derive(Debug)]
struct Scored<'a> {
    session: &'a SessionRecord,
    retention: RetentionOutcome,
    quadrant: Quadrant,
    cost_usd: f64,
    pinpoints: Vec<WastePinpoint>,
}

// ---------------------------------------------------------------------------
// Section renderers
// ---------------------------------------------------------------------------

fn render_pinpoints(pinpoints: &[&WastePinpoint]) -> String {
    if pinpoints.is_empty() {
        return String::from(
            r#"<section class="card">
  <h2>Where time was wasted</h2>
  <p class="muted">No waste pinpoints in this window. Either a great week,
  or sessions haven't aged into their retention window yet.</p>
</section>"#,
        );
    }
    let mut rows = String::new();
    for p in pinpoints.iter().take(10) {
        let severity_class = match p.severity {
            PinpointSeverity::Severe => "sev-severe",
            PinpointSeverity::Iterated => "sev-iterated",
            PinpointSeverity::Lost => "sev-lost",
        };
        let explanation = explain_pinpoint(p);
        rows.push_str(&format!(
            r#"
      <tr class="{severity_class}">
        <td><span class="badge {severity_class}">{label}</span></td>
        <td><code>{file}</code></td>
        <td class="num">{edits}</td>
        <td class="num">{lines_added}</td>
        <td class="num">{retention:.0}%</td>
        <td class="muted small">{project}</td>
        <td class="muted small"><code class="sid">{sid}</code></td>
        <td class="explain">{explanation}</td>
      </tr>"#,
            label = p.severity.label(),
            file = escape(&display_path(&p.file)),
            edits = p.edits,
            lines_added = p.lines_added,
            retention = p.retention * 100.0,
            project = escape(p.project.as_deref().unwrap_or("-")),
            sid = escape(short_id(&p.session_id)),
            explanation = escape(&explanation),
        ));
    }
    format!(
        r#"<section class="card pinpoints">
  <h2>Where time was wasted</h2>
  <p class="muted">The top files that absorbed edits and didn't survive in HEAD. Click a session ID to drill in.</p>
  <table class="data">
    <thead><tr>
      <th>Severity</th><th>File</th><th>Edits</th><th>Lines+</th><th>Retention</th><th>Project</th><th>Session</th><th>Why</th>
    </tr></thead>
    <tbody>{rows}</tbody>
  </table>
</section>"#,
        rows = rows
    )
}

fn render_drivers(sessions: &[SessionRecord]) -> String {
    use crate::correlate::{baseline, group_by_driver, DriverKey};
    let groups = group_by_driver(sessions);
    if groups.is_empty() {
        return String::from(
            r#"<section class="card">
  <h2>Drivers</h2>
  <p class="muted">No groups large enough yet (need ≥3 sessions per driver).
  As more sessions accumulate, this section will show which skills, CLAUDE.md
  versions, hooks, and edit patterns correlate with higher retention.</p>
</section>"#,
        );
    }
    let base = baseline(sessions);
    let base_ret_pct = base
        .avg_retention
        .map(|r| format!("{:.0}%", r * 100.0))
        .unwrap_or_else(|| "-".to_string());

    // Group rows by driver type so the section reads as one subsection per type.
    use std::collections::BTreeMap;
    let mut by_type: BTreeMap<&'static str, Vec<&crate::correlate::DriverGroup>> = BTreeMap::new();
    for g in &groups {
        by_type.entry(g.key.type_slug()).or_default().push(g);
    }

    let mut body = String::new();
    for (slug, gs) in &by_type {
        let label = match *slug {
            "skill" => "By skill",
            "claude_md" => "By CLAUDE.md version",
            "hook_event" => "By hook event",
            "model" => "By model",
            "edit_pattern" => "By edit pattern (Read before Edit?)",
            other => other,
        };
        let mut rows = String::new();
        for g in gs {
            let key_display = match &g.key {
                DriverKey::Skill(s) => s.clone(),
                DriverKey::ClaudeMdHash(h) => format!("#{}", &h[..h.len().min(8)]),
                DriverKey::HookEvent(e) => e.clone(),
                DriverKey::Model(m) => m.clone(),
                DriverKey::EditWithoutPriorRead(true) => "edit without prior read".to_string(),
                DriverKey::EditWithoutPriorRead(false) => "read first".to_string(),
            };
            let retention_cell = match (g.avg_retention, base.avg_retention) {
                (Some(r), Some(b)) => format!(
                    "{:.0}% <span class=\"muted small\">(Δ {:+.0}pt)</span>",
                    r * 100.0,
                    (r - b) * 100.0
                ),
                (Some(r), None) => format!("{:.0}%", r * 100.0),
                (None, _) => "-".to_string(),
            };
            let cost_delta = if base.avg_cost > 0.0 {
                format!(
                    "<span class=\"muted small\">(Δ {:+.0}%)</span>",
                    (g.avg_cost - base.avg_cost) / base.avg_cost * 100.0
                )
            } else {
                String::new()
            };
            let url_key = percent_encode_path(&g.key.display_value());
            rows.push_str(&format!(
                r#"
      <tr hx-get="/driver/{slug}/{url_key}" hx-target="next .driver-detail" hx-swap="innerHTML">
        <td>{key}</td>
        <td class="num">{n}</td>
        <td class="num">{retention_cell}</td>
        <td class="num">${cost:.4} {cost_delta}</td>
      </tr>
      <tr class="driver-detail-row"><td colspan="4" class="driver-detail"></td></tr>"#,
                slug = slug,
                key = escape(&key_display),
                n = g.n,
                cost = g.avg_cost,
            ));
        }
        body.push_str(&format!(
            r#"
  <h3>{label}</h3>
  <table class="data drivers">
    <thead><tr><th>Group</th><th>N</th><th>Avg retention</th><th>Avg cost</th></tr></thead>
    <tbody>{rows}</tbody>
  </table>"#,
            label = escape(label),
        ));
    }

    format!(
        r#"<section class="card drivers">
  <h2>Drivers</h2>
  <p class="muted">
    Sessions grouped by environment driver. Baseline avg retention: {base_ret_pct} across {base_n} session(s).
    Groups under 3 sessions are hidden as noise. Correlation, not causation —
    pin a CLAUDE.md hash and toggle one feature to compare cleanly.
  </p>
  {body}
</section>"#,
        base_n = base.n,
    )
}

/// Percent-encode characters that would break a URL path segment. Simple
/// targeted encoder — we accept ASCII alphanumerics, `-`, `_`, `.`, `@`
/// (model + plugin keys use it) and percent-encode the rest as UTF-8.
fn percent_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'@') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn explain_pinpoint(p: &WastePinpoint) -> String {
    match p.severity {
        PinpointSeverity::Severe => format!(
            "{} edits absorbed; only {:.0}% of the diff survives in HEAD. Look here first.",
            p.edits,
            p.retention * 100.0
        ),
        PinpointSeverity::Iterated => format!(
            "Iterated {}× and lost {:.0}% of the work. Candidate for prompt refactor.",
            p.edits,
            (1.0 - p.retention) * 100.0
        ),
        PinpointSeverity::Lost => format!(
            "{} lines written, none survive. Either reverted or rewritten by hand.",
            p.lines_added
        ),
    }
}

fn render_quadrant(scored: &[Scored]) -> String {
    let mut counts: BTreeMap<&'static str, u32> = BTreeMap::new();
    for s in scored {
        *counts.entry(s.quadrant.label()).or_default() += 1;
    }
    let q = |k: &str| counts.get(k).copied().unwrap_or(0);
    format!(
        r#"<section class="card">
  <h2>Quadrant</h2>
  <div class="quadrant">
    <div class="cell qw"><span class="label">QUICK WIN</span><span class="big">{qw}</span><span class="muted small">short · kept</span></div>
    <div class="cell dv"><span class="label">DEEP VALUE</span><span class="big">{dv}</span><span class="muted small">long · kept</span></div>
    <div class="cell cw"><span class="label">CHEAP WASTE</span><span class="big">{cw}</span><span class="muted small">short · lost</span></div>
    <div class="cell ew"><span class="label">EXPENSIVE WASTE</span><span class="big">{ew}</span><span class="muted small">long · lost</span></div>
  </div>
  <p class="other muted small">Other: PENDING {pe} · REBASED {re} · UNMEASURABLE {un}</p>
</section>"#,
        qw = q("QUICK WIN"),
        dv = q("DEEP VALUE"),
        cw = q("CHEAP WASTE"),
        ew = q("EXPENSIVE WASTE"),
        pe = q("PENDING"),
        re = q("REBASED"),
        un = q("UNMEASURABLE"),
    )
}

fn render_top_value(scored: &[Scored]) -> String {
    let mut copy: Vec<&Scored> = scored.iter().collect();
    copy.sort_by(|a, b| value_score(b).partial_cmp(&value_score(a)).unwrap());
    let copy: Vec<&Scored> = copy
        .into_iter()
        .filter(|s| matches!(s.retention, RetentionOutcome::Scored(r) if r >= 0.5))
        .take(3)
        .collect();
    if copy.is_empty() {
        return String::new();
    }
    let mut rows = String::new();
    for s in copy {
        rows.push_str(&format_session_row(s, "value"));
    }
    format!(
        r#"<section class="card">
  <h2>What worked</h2>
  <table class="data">
    <thead><tr><th>Session</th><th>Project</th><th>Quadrant</th><th>Lines+</th><th>Retention</th><th>Cost</th><th>Duration</th></tr></thead>
    <tbody>{rows}</tbody>
  </table>
</section>"#
    )
}

fn value_score(s: &Scored) -> f64 {
    let r = match s.retention {
        RetentionOutcome::Scored(r) => r,
        _ => 0.0,
    };
    r * s.session.lines_added as f64
}

fn render_sessions(scored: &[Scored]) -> String {
    if scored.is_empty() {
        return String::from(
            r#"<section class="card"><h2>Sessions</h2><p class="muted">None captured in window.</p></section>"#,
        );
    }
    let mut rows = String::new();
    for s in scored.iter().take(50) {
        rows.push_str(&format_session_row(s, "all"));
    }
    format!(
        r#"<section class="card">
  <h2>All sessions <span class="muted small">(newest first, capped at 50)</span></h2>
  <table class="data sessions">
    <thead><tr><th>Session</th><th>Project</th><th>Quadrant</th><th>Lines+</th><th>Retention</th><th>Cost</th><th>Duration</th></tr></thead>
    <tbody>{rows}</tbody>
  </table>
</section>"#
    )
}

fn format_session_row(s: &Scored, kind: &str) -> String {
    let q_class = match s.quadrant {
        Quadrant::QuickWin => "q-qw",
        Quadrant::DeepValue => "q-dv",
        Quadrant::CheapWaste => "q-cw",
        Quadrant::ExpensiveWaste => "q-ew",
        Quadrant::Pending => "q-pe",
        Quadrant::Rebased => "q-re",
        Quadrant::Unmeasurable => "q-un",
    };
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
        r#"<tr class="row-{kind}" hx-get="/session/{sid}" hx-target="next .session-detail" hx-swap="innerHTML">
  <td><code class="sid">{short}</code></td>
  <td>{project}</td>
  <td><span class="badge {q_class}">{q_label}</span></td>
  <td class="num">{added}</td>
  <td class="num">{retention}</td>
  <td class="num">${cost:.2}</td>
  <td class="num">{duration}</td>
</tr>
<tr class="detail-row"><td colspan="7"><div class="session-detail"></div></td></tr>"#,
        sid = escape(&s.session.session_id),
        short = escape(short_id(&s.session.session_id)),
        project = escape(s.session.project.as_deref().unwrap_or("-")),
        q_label = s.quadrant.label(),
        added = s.session.lines_added,
        cost = s.cost_usd,
    )
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

    // Window-aggregate cache hit ratio. Hidden when no LLM activity exists.
    let cache_read: u64 = scored.iter().map(|s| s.session.cache_read_tokens).sum();
    let cache_create: u64 = scored.iter().map(|s| s.session.cache_creation_tokens).sum();
    let inputs: u64 = scored.iter().map(|s| s.session.input_tokens).sum();
    let cache_denom = cache_read + cache_create + inputs;
    let cache_row = if cache_denom == 0 {
        String::new()
    } else {
        let pct = 100.0 * cache_read as f64 / cache_denom as f64;
        format!(
            r#"    <tr><th>Cache hit ratio</th><td>{pct:.0}% <span class="muted">({cache_read} read / {cache_denom} total)</span></td></tr>
"#
        )
    };

    // Sessions by model — hidden when no session has model set.
    let mut by_model: BTreeMap<&str, u32> = Default::default();
    for s in scored {
        if let Some(m) = s.session.model.as_deref() {
            *by_model.entry(m).or_default() += 1;
        }
    }
    let model_row = if by_model.is_empty() {
        String::new()
    } else {
        let parts: Vec<String> = by_model
            .iter()
            .map(|(m, n)| format!("{}: {}", escape(m), n))
            .collect();
        format!(
            r#"    <tr><th>Sessions by model</th><td>{}</td></tr>
"#,
            parts.join(", ")
        )
    };

    format!(
        r#"<section class="card">
  <h2>Totals</h2>
  <table class="data kv">
    <tr><th>Total cost</th><td>${total_cost:.2}</td></tr>
    <tr><th>Claude $ cost</th><td>${total_claude_cost:.4}</td></tr>
    <tr><th>Total duration</th><td>{total_minutes:.0} min</td></tr>
    <tr><th>Total turns</th><td>{total_turns}</td></tr>
    <tr><th>Lines added</th><td>{total_added}</td></tr>
    <tr><th>Lines removed</th><td>{total_removed}</td></tr>
    <tr><th>Lines surviving in HEAD</th><td>{surviving} ({retention_pct:.0}%)</td></tr>
{cache_row}{model_row}  </table>
</section>"#
    )
}

fn render_by_project(scored: &[Scored]) -> String {
    let mut groups: BTreeMap<String, Vec<&Scored>> = BTreeMap::new();
    for s in scored {
        let key = s.session.project.clone().unwrap_or_else(|| "-".to_string());
        groups.entry(key).or_default().push(s);
    }
    let mut rows = String::new();
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
        rows.push_str(&format!(
            "<tr><td>{}</td><td class=\"num\">{}</td><td class=\"num\">${cost:.2}</td><td class=\"num\">{added}</td><td class=\"num\">{surviving}</td></tr>",
            escape(&proj),
            ss.len(),
        ));
    }
    format!(
        r#"<section class="card">
  <h2>By project</h2>
  <table class="data">
    <thead><tr><th>Project</th><th>Sessions</th><th>Cost</th><th>Lines+</th><th>Surviving</th></tr></thead>
    <tbody>{rows}</tbody>
  </table>
</section>"#
    )
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn oldest_label(scored: &[Scored]) -> String {
    scored
        .iter()
        .filter_map(|s| s.session.started_at)
        .min()
        .map(|t| format!("since {}", t.format("%Y-%m-%d")))
        .unwrap_or_else(|| "window".to_string())
}

fn short_id(s: &str) -> &str {
    &s[..s.len().min(8)]
}

fn display_path(p: &str) -> String {
    // Trim to last 3 segments for readability in tables.
    let parts: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() <= 3 {
        p.to_string()
    } else {
        format!("…/{}", parts[parts.len() - 3..].join("/"))
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ---------------------------------------------------------------------------
// Style (embedded so the output is self-contained)
// ---------------------------------------------------------------------------

const STYLE: &str = r#"
:root {
  color-scheme: dark light;
  --bg: #0e0f12;
  --bg-card: #16181d;
  --bg-card-2: #1c1f25;
  --fg: #e6e6e6;
  --fg-muted: #8a8f99;
  --border: #2a2e36;
  --accent: #6aa1ff;
  --severe: #ff5c5c;
  --iterated: #ffb454;
  --lost: #ff8080;
  --qw: #61d28a;
  --dv: #6aa1ff;
  --cw: #ffb454;
  --ew: #ff5c5c;
  font-feature-settings: "tnum";
}
* { box-sizing: border-box; }
body {
  margin: 0; padding: 0;
  font: 14px/1.5 ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif;
  background: var(--bg);
  color: var(--fg);
}
.topbar {
  position: sticky; top: 0; z-index: 10;
  display: flex; justify-content: space-between; align-items: center;
  padding: 12px 24px; border-bottom: 1px solid var(--border);
  background: var(--bg);
}
.brand { font-weight: 600; letter-spacing: 0.02em; }
.meta { color: var(--fg-muted); font-variant-numeric: tabular-nums; }
main { padding: 24px; max-width: 1200px; margin: 0 auto; }
.card {
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 20px 24px;
  margin-bottom: 20px;
}
h2 { margin: 0 0 12px 0; font-size: 14px; text-transform: uppercase; letter-spacing: 0.08em; color: var(--fg-muted); }
.muted { color: var(--fg-muted); }
.small { font-size: 12px; }
.num { font-variant-numeric: tabular-nums; text-align: right; }
code { font: 12px/1.4 ui-monospace, "SF Mono", Menlo, monospace; }
code.sid { color: var(--accent); }
table.data { width: 100%; border-collapse: collapse; }
table.data th, table.data td { padding: 6px 8px; text-align: left; border-bottom: 1px solid var(--border); vertical-align: top; }
table.data th { font-weight: 500; color: var(--fg-muted); }
table.data.kv th { width: 220px; }
table.sessions tbody tr:not(.detail-row):hover { background: var(--bg-card-2); cursor: pointer; }
table.sessions .detail-row td { padding: 0; border: none; }
.session-detail:empty { display: none; }
.session-detail { padding: 12px 16px; background: var(--bg-card-2); border-bottom: 1px solid var(--border); }
table.drivers tbody tr:not(.driver-detail-row):hover { background: var(--bg-card-2); cursor: pointer; }
table.drivers .driver-detail-row td { padding: 0; border: none; }
.driver-detail:empty { display: none; }
.driver-detail { padding: 10px 14px; background: var(--bg-card-2); border-bottom: 1px solid var(--border); }
section.drivers h3 { margin-top: 16px; font-size: 12px; text-transform: uppercase; letter-spacing: 0.06em; color: var(--fg-muted); }
.quadrant {
  display: grid; grid-template-columns: 1fr 1fr; gap: 12px;
  margin: 6px 0;
}
.cell {
  padding: 16px; border-radius: 6px; background: var(--bg-card-2);
  display: flex; flex-direction: column; gap: 4px;
  border-left: 3px solid var(--border);
}
.cell.qw { border-left-color: var(--qw); }
.cell.dv { border-left-color: var(--dv); }
.cell.cw { border-left-color: var(--cw); }
.cell.ew { border-left-color: var(--ew); }
.cell .label { font-size: 11px; letter-spacing: 0.06em; color: var(--fg-muted); }
.cell .big { font-size: 28px; font-weight: 600; font-variant-numeric: tabular-nums; }
.other { margin-top: 8px; }
.badge {
  display: inline-block; padding: 2px 8px; border-radius: 10px;
  font-size: 11px; letter-spacing: 0.04em;
  background: var(--bg-card-2); color: var(--fg-muted); border: 1px solid var(--border);
}
.badge.sev-severe { color: #fff; background: var(--severe); border-color: var(--severe); }
.badge.sev-iterated { color: #2a1c00; background: var(--iterated); border-color: var(--iterated); }
.badge.sev-lost { color: #fff; background: var(--lost); border-color: var(--lost); }
.badge.q-qw { color: #06351a; background: var(--qw); border-color: var(--qw); }
.badge.q-dv { color: #fff; background: var(--dv); border-color: var(--dv); }
.badge.q-cw { color: #2a1c00; background: var(--cw); border-color: var(--cw); }
.badge.q-ew { color: #fff; background: var(--ew); border-color: var(--ew); }
.explain { color: var(--fg-muted); font-size: 12px; max-width: 36ch; }
.caveats {
  padding: 18px 24px; max-width: 1200px; margin: 0 auto;
  color: var(--fg-muted); font-size: 12px; border-top: 1px solid var(--border);
}
.caveats strong { color: var(--fg); }
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn mk(id: &str, project: &str, added: u32, cost: f64) -> SessionRecord {
        let mut s = SessionRecord::default();
        s.session_id = id.to_string();
        s.project = Some(project.to_string());
        s.cwd = "/no/such/path".to_string();
        s.lines_added = added;
        s.total_cost_usd = cost;
        let t = Utc.with_ymd_and_hms(2026, 6, 7, 12, 0, 0).unwrap();
        s.started_at = Some(t);
        s.ended_at = Some(t + chrono::Duration::minutes(5));
        s
    }

    #[test]
    fn renders_minimum_sections() {
        let s = vec![mk("abcd1234", "demo", 100, 0.50)];
        let html = render(&s, &Config::default(), Some("project"));
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("Where time was wasted")); // pinpoint section
        assert!(html.contains("Quadrant"));
        assert!(html.contains("Totals"));
        assert!(html.contains("By project"));
        assert!(html.contains("htmx.org"));
        assert!(html.contains("abcd1234"));
    }

    #[test]
    fn escapes_html_in_paths() {
        let html = escape("<script>x</script>");
        assert_eq!(html, "&lt;script&gt;x&lt;/script&gt;");
    }

    #[test]
    fn empty_render_shows_no_waste_message() {
        let html = render(&[], &Config::default(), None);
        assert!(html.contains("No waste pinpoints in this window"));
    }
}
