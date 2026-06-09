---
name: claude-time-report
description: Use when the user asks "is Claude saving me time", "show my claude-time report", or wants to interpret a claude-time markdown report. Runs `claude-time report` with the right flags, then explains the quadrant + non-scored outcomes in plain English.
---

# claude-time-report

## When this skill fires

The user is asking about ROI on their Claude Code usage. Likely phrasings:

- "is Claude saving me time?"
- "show my claude-time report"
- "what's my retention rate?"
- "where am I wasting tokens?"
- "explain this report"

## Run the report

Default: last 7 days.

```sh
claude-time report --since 7d
```

Useful variants:

```sh
claude-time report --since 30d --by project   # monthly, per-project
claude-time report --since 90d                # quarterly
claude-time status                            # is the tracker installed?
```

If `claude-time` is not on PATH, suggest: `cargo install claude-time` or
direct the user to https://github.com/ayodm/claude-time#install.

## How to explain the quadrant

The report classifies each session into one of four cells:

| Quadrant | Meaning |
|---|---|
| **QUICK WIN** | Short session, diff still in HEAD. Cheap value. |
| **DEEP VALUE** | Long session, diff still in HEAD. Earned its cost. |
| **CHEAP WASTE** | Short session, diff reverted/rewritten. Hallucination cost, but cheap. |
| **EXPENSIVE WASTE** | Long session, diff gone. The signal worth examining. |

Plus three non-scored outcomes the user should understand:

- **PENDING** — session not yet old enough; retention window hasn't elapsed
- **REBASED** — session's commit was squashed/rebased away; signal lost
- **UNMEASURABLE** — session ran outside a git repo

## What the report does NOT measure

Always remind the user — the footer says it, but it's important:

- No absolute "time saved" claim (no baseline)
- No code quality measure beyond retention
- No subjective satisfaction
- No learning value (a session that taught something has long-tail value
  retention can't see)

## Common follow-ups

- **"What can I do about EXPENSIVE WASTE sessions?"** Look at the session
  IDs in "Top waste", read the prompts in the transcript at
  `~/.claude/projects/<project>/<session-id>.jsonl`, find the pattern.
- **"My retention is low — is Claude bad?"** Could be: aggressive squash
  workflow (look for high REBASED count), exploratory work that legitimately
  iterates, or genuine quality issues. Compare different `--by project` to
  isolate.
- **"How do I add my hourly rate?"** Edit `~/.claude/claude-time/config.toml`,
  set `[report] hourly_rate_usd = <rate>`.
