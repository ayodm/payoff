---
name: payoff-report
description: Use when the user asks "did my AI session pay off?", "show my payoff report", "where am I wasting time?", or wants to interpret a payoff HTML report. Runs `payoff report` with the right flags, opens the HTML in their browser, then explains the waste pinpoints + quadrant in plain English.
---

# payoff-report

`payoff` was previously published as `claude-time` — the migration is
handled automatically on first install (data dir + hook entries migrate).

## When this skill fires

The user is asking about ROI on their AI Code session. Likely phrasings:

- "did this session pay off?"
- "show my payoff report"
- "where am I wasting time?"
- "what's my retention rate?"
- "explain this report"

## Run the report

Default: writes self-contained HTML to `~/.claude/payoff/last-report.html`
and opens it in the browser.

```sh
payoff report --since 7d
```

Variants:

```sh
payoff report --since 30d --by project   # monthly, per-project
payoff report --serve --port 7878        # live HTMX-driven server
payoff report --stdout                   # HTML to stdout (CI / piping)
payoff report --markdown                 # legacy markdown for terminal
payoff status                            # is the tracker installed?
```

If `payoff` is not on PATH, suggest:
`cargo install payoff` or
`/plugin marketplace add ayodm/payoff` then
`/plugin install payoff@payoff`
(see https://github.com/ayodm/payoff#install).

## How to explain the report

The HTML report leads with **"Where time was wasted"** — that's the answer
to the most common question. Walk the user through the top 3 pinpoints:

- **SEVERE** badge — 5+ edits on a single file with <10% retention. Highest
  priority. "Look here first."
- **ITERATED** badge — 3+ edits, <50% retention. Visible churn that didn't
  stick. Often a prompt-refactor candidate.
- **LOST** badge — single edit, full retention loss. File was written, then
  reverted or rewritten by hand.

Each pinpoint shows the explanation column ("Why") in plain English — read
that to the user.

Then the **Drivers** section groups sessions by environment feature (which
skills were active, which CLAUDE.md hash, which model, edit pattern) and
shows retention/cost deltas vs the all-sessions baseline. Use this to
answer "did changing X help?" — but flag that it's correlation, not
causation.

Then the **Quadrant** block summarizes whole sessions:

| Quadrant | Meaning |
|---|---|
| **QUICK WIN** | Short session, diff still in HEAD. Cheap value. |
| **DEEP VALUE** | Long session, diff still in HEAD. Earned its cost. |
| **CHEAP WASTE** | Short session, diff reverted/rewritten. Cheap but unproductive. |
| **EXPENSIVE WASTE** | Long session, diff gone. The signal worth examining. |

Plus three non-scored outcomes:

- **PENDING** — session not yet old enough; retention window hasn't elapsed
- **REBASED** — session's commit was squashed/rebased away; signal lost
- **UNMEASURABLE** — session ran outside a git repo

## Server mode (`--serve`)

If the user wants to *explore* rather than just read, start the server:

```sh
payoff report --serve
```

This opens an HTMX-driven page where clicking a session row expands to show
per-file pinpoints, tool-call mix, exact tokens, full cwd. Clicking a
driver row drills into the sessions in that group. Useful for debugging a
specific bad session.

## What the report does NOT measure

Always remind the user — the footer says it, but it's important:

- No absolute "time saved" claim (no baseline)
- No code quality measure beyond retention
- No subjective satisfaction
- No learning value (a session that taught something has long-tail value
  retention can't see)
- Correlations in the Drivers section are not causation — pin a CLAUDE.md
  hash and toggle one feature to compare cleanly.

## Common follow-ups

- **"What can I do about EXPENSIVE WASTE sessions?"** Look at the session
  IDs at the top of the pinpoint table, find the transcripts at
  `~/.claude/projects/<project>/<session-id>.jsonl`, read the prompts that
  produced the wasted file. The pattern almost always rhymes across
  sessions — there's a particular kind of task that Claude isn't nailing
  for you, and tightening the prompt is usually the fix.
- **"My retention is low — is Claude bad?"** Could be: aggressive squash
  workflow (high REBASED count signals this), exploratory work that
  legitimately iterates, or genuine quality issues. Run
  `payoff report --by project` to isolate which projects are dragging
  the number down.
- **"How do I add my hourly rate?"** Edit `~/.claude/payoff/config.toml`,
  set `[report] hourly_rate_usd = <rate>`. The cost column will then include
  your time.
- **"Where do the session transcripts live?"**
  `~/.claude/projects/<project>/<session-id>.jsonl`. Claude Code's default
  retention is 30 days; configure via `cleanupPeriodDays` in your
  `~/.claude/settings.json`.
