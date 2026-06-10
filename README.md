# payoff

[![CI](https://github.com/ayodm/payoff/actions/workflows/ci.yml/badge.svg)](https://github.com/ayodm/payoff/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/payoff.svg)](https://crates.io/crates/payoff)
[![crates.io downloads](https://img.shields.io/crates/d/payoff.svg)](https://crates.io/crates/payoff)
[![license](https://img.shields.io/crates/l/payoff.svg)](LICENSE)

> ```sh
> cargo install payoff
> ```
>
> 107 unit tests + 16 simulated lifecycle scenarios (every realistic
> SessionStart/SessionEnd flow plus adversarial inputs: path-traversal
> session IDs, malformed settings.json, mid-session CLAUDE.md rewrites,
> concurrent sessions in different repos, XSS via stored fields) all
> pass. The SessionStart/SessionEnd hooks are confirmed firing from
> inside real Claude Code sessions, with driver capture (model, CLAUDE.md
> hashes, enabled plugins, plugin-bundled skills) populating live records.

Passive ROI tracker for AI coding sessions. Did the session pay off?

Most tools tell you *what you did* with AI. None tell you *whether it was
worth it*. `payoff` answers the second question by substituting
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
- **Waste pinpoints** — for each session, which *specific files* absorbed
  many edits but didn't survive in HEAD. Three severity tiers:
  - `SEVERE` — 5+ edits, <10% retention. Look here first.
  - `ITERATED` — 3+ edits, <50% retention. Prompt-refactor candidate.
  - `LOST` — file written, nothing survived. Reverted or rewritten by hand.
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

Three routes — pick whichever fits.

**A. Rust users (smallest binary, latest version)**

```sh
cargo install payoff
payoff install
```

**B. No Rust toolchain (pre-built binary)**

```sh
curl -fsSL https://raw.githubusercontent.com/ayodm/payoff/main/installer.sh | bash
payoff install
```

**C. Claude Code plugin (hooks bundled, you still install the binary)**

```sh
# In Claude Code:
/plugin marketplace add ayodm/payoff
/plugin install payoff@payoff

# Then on your shell:
cargo install payoff          # or use route B
```

The plugin registers the SessionStart + SessionEnd hooks and ships a
companion skill (`payoff-report`) that explains the markdown report on
demand. You still need the binary on `$PATH` for the hooks to do anything.

Verify any of the above:

```sh
payoff status
```

## Use

```sh
payoff report                       # writes HTML + opens browser
payoff report --since 30d           # window control
payoff report --by project          # group by project
payoff report --retention-window 14
payoff report --serve               # HTMX-driven local server
payoff report --stdout              # HTML to stdout (CI / piping)
payoff report --markdown            # legacy terminal-readable
payoff archive                      # compact old sessions on demand
payoff uninstall                    # remove hooks; keep data dir
```

## Storage

`payoff` stores data under `~/.claude/payoff/`:

```
sessions/<session_id>.json      # in-flight + recent closed sessions
archive.jsonl                   # closed sessions older than retention window
cache/retention/                # (v0.2)
config.toml                     # user overrides
```

**Disk footprint.** A compact session record is ~250 bytes. The hot path
(per-session files) costs one 4 KB filesystem block per session — fine for
the most recent week, wasteful for years of history. So `payoff report`
runs an opportunistic archive: closed sessions older than the retention
window get rolled into a single append-only `archive.jsonl`, the per-session
files are deleted, and block overhead vanishes.

Measured (100 sessions, macOS APFS):

| | On disk |
|---|---|
| Per-session files | 400 KB |
| `archive.jsonl` after rollup | 20 KB |

Heavy use (~50 sessions/day) projects to **~5 MB/year** of total payoff
data. The Claude Code transcripts themselves (under `~/.claude/projects/`)
are typically much larger; set `cleanupPeriodDays` in your Claude Code
settings if those bother you — payoff only needs the transcripts during
the session that produced them.

## Config

`~/.claude/payoff/config.toml` (created on first edit):

```toml
[report]
retention_window_days = 7
hourly_rate_usd = 0.0     # 0.0 = payoff reports only $ cost from Claude
default_period = "7d"

[exclude]
paths = [".git", "node_modules", "target", "dist", ".next", "_build", "deps"]
```

`hourly_rate_usd > 0` adds your time cost into the cost-side of the quadrant
(useful if you want to compare against your own effective hourly rate).

## Adoption

payoff is intentionally untelemetered — no install beacon, no upload of
session data, nothing leaves your machine. Adoption shows up only via
registry-native counts:

- crates.io download counts on the [package page](https://crates.io/crates/payoff)
- GitHub Release asset download counts on each [release](https://github.com/ayodm/payoff/releases)
- GitHub stars / forks / traffic insights

If you find it useful, a star on GitHub is the most direct signal.

## Honest caveats

- **Retention is a proxy, not the truth.** A session that taught you
  something you reused tomorrow has positive value even if its specific
  diff was reverted. The report's "learning value" footer acknowledges
  this.
- **Rebases destroy the signal.** If you squash before merging, payoff
  marks affected sessions as `REBASED` rather than guessing. You'll see
  `REBASED` counts in the report; tune your retention window or skip the
  squash if it matters to you.
- **No baseline, no time-saved claim.** v0.2 will add an opt-in 1-tap
  estimation slider at session start if you want it. The default stays
  passive-only.

## License

MIT
