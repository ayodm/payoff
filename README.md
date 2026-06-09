# claude-time

Passive-only ROI tracker for Claude Code sessions.

Most tools tell you *what you did* with AI. None tell you *whether it was
worth it*. `claude-time` answers the second question by substituting
**diff retention** for the unknowable "time saved" baseline: a session's
value is proxied by whether its diff survived in the codebase over the
next N days.

Zero user-facing prompts. One static binary. ~5 MB/year of disk for a
heavy user.

## What it measures

Captured passively via Claude Code hooks:

- Session token spend + dollar cost (per session, per model)
- Duration, turn count, tool-call mix
- Files touched and the lines added/removed (`git diff sha_before..sha_after`)

Scored at report time:

- **Retention** — what fraction of a session's diff still exists in HEAD
  after N days (default 7). Computed via `git blame` against the session's
  commit.
- **Quadrant** — each session classified as:
  - `QUICK WIN` — high retention, low cost
  - `DEEP VALUE` — high retention, high cost
  - `CHEAP WASTE` — low retention, low cost (hallucination cost, but cheap)
  - `EXPENSIVE WASTE` — low retention, high cost (the one to look at)

Or one of the explicit non-scored outcomes:

- `PENDING` — session not yet old enough for retention to be meaningful
- `REBASED` — the session's commit was rebased/squashed away (blame signal lost)
- `UNMEASURABLE` — session ran outside a git repo

## What it deliberately does NOT measure (v0.1)

- Absolute time saved (no user-provided baseline → no ground truth)
- Code quality beyond retention (no static analysis, no test results)
- Subjective satisfaction (no prompts, no thumbs)
- Learning value (a session that taught you something has long-tail value
  retention can't see)

Listed honestly here and in the report footer so you remember what you're
looking at.

## Install

```sh
cargo install claude-time
claude-time install      # patches ~/.claude/settings.json with hooks
```

Verify:

```sh
claude-time status
```

## Use

```sh
claude-time report                       # last 7 days, markdown to stdout
claude-time report --since 30d
claude-time report --by project
claude-time report --retention-window 14
claude-time archive                      # compact old sessions on demand
claude-time uninstall                    # remove hooks; keep data dir
```

## Storage

`claude-time` stores data under `~/.claude/claude-time/`:

```
sessions/<session_id>.json      # in-flight + recent closed sessions
archive.jsonl                   # closed sessions older than retention window
cache/retention/                # (v0.2)
config.toml                     # user overrides
```

**Disk footprint.** A compact session record is ~250 bytes. The hot path
(per-session files) costs one 4 KB filesystem block per session — fine for
the most recent week, wasteful for years of history. So `claude-time report`
runs an opportunistic archive: closed sessions older than the retention
window get rolled into a single append-only `archive.jsonl`, the per-session
files are deleted, and block overhead vanishes.

Measured (100 sessions, macOS APFS):

| | On disk |
|---|---|
| Per-session files | 400 KB |
| `archive.jsonl` after rollup | 20 KB |

Heavy use (~50 sessions/day) projects to **~5 MB/year** of total claude-time
data. The Claude Code transcripts themselves (under `~/.claude/projects/`)
are typically much larger; set `cleanupPeriodDays` in your Claude Code
settings if those bother you — claude-time only needs the transcripts during
the session that produced them.

## Config

`~/.claude/claude-time/config.toml` (created on first edit):

```toml
[report]
retention_window_days = 7
hourly_rate_usd = 0.0     # 0.0 = claude-time reports only $ cost from Claude
default_period = "7d"

[exclude]
paths = [".git", "node_modules", "target", "dist", ".next", "_build", "deps"]
```

`hourly_rate_usd > 0` adds your time cost into the cost-side of the quadrant
(useful if you want to compare against your own effective hourly rate).

## Honest caveats

- **Retention is a proxy, not the truth.** A session that taught you
  something you reused tomorrow has positive value even if its specific
  diff was reverted. The report's "learning value" footer acknowledges
  this.
- **Rebases destroy the signal.** If you squash before merging, claude-time
  marks affected sessions as `REBASED` rather than guessing. You'll see
  `REBASED` counts in the report; tune your retention window or skip the
  squash if it matters to you.
- **No baseline, no time-saved claim.** v0.2 will add an opt-in 1-tap
  estimation slider at session start if you want it. The default stays
  passive-only.

## License

MIT
