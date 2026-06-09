# claude-time — current state

This file auto-loads when a Claude Code session starts in this directory.
Read it first.

---

## What this project is

A passive ROI tracker for Claude Code itself. Measures whether Claude is
*saving you time* by substituting **diff retention** for the unknowable
"time saved" baseline. A session's diff that still exists in HEAD after N
days = value; a diff that was reverted or rewritten = waste.

- **Repo:** https://github.com/ayodm/claude-time (public)
- **Latest:** v0.1.1 on `main`. Five commits, all author `Ayo M <ayodm@me.com>`.
- **Stack:** Rust (single binary, no runtime deps for users)
- **Distribution:** crates.io + GitHub Releases + Claude Code plugin marketplace + Homebrew tap (planned)
- **License:** MIT
- **Commit identity for this repo:** `Ayo M <ayodm@me.com>`. The gmail
  address must NOT appear in commits. Local git config is set; verify with
  `git config user.email`.
- **Installed on this machine:** `~/.cargo/bin/claude-time` (v0.1.1) with
  hooks active in `~/.claude/settings.json`. Backup of pre-install
  settings at `~/.claude/settings.json.before-claude-time.bak`.

## What's shipped

### v0.1.0 — initial release

- Passive SessionStart/SessionEnd capture (`src/hooks.rs`)
- Retention scoring via libgit2 blame (`src/git_history.rs`)
- 4-cell quadrant + 3 non-scored outcomes (`src/model.rs`)
- Markdown report (`src/report.rs`)
- Storage compaction — per-session JSON + rollup to `archive.jsonl`,
  measured 20x disk savings (`src/storage.rs`)
- Install/uninstall with non-destructive `settings.json` patching
  (`src/install.rs`)
- Plugin manifest + marketplace catalog + hooks declaration + companion
  skill in `.claude-plugin/`, `hooks/`, `skills/`
- CI workflow + release workflow (`.github/workflows/`)
- Installer.sh for non-Rust users

### v0.1.1 — HTML/HTMX + waste pinpoints

- **Default report is now self-contained HTML** that opens in the browser
  (`src/html_report.rs`). Pinpoint section leads — answers "where am I
  wasting time" before the per-session table.
- **Per-file waste pinpoints** — every session computes a ranked list of
  files that absorbed edits but didn't survive. Three severity tiers:
  `SEVERE` (5+ edits, <10% retention), `ITERATED` (3+ edits, <50%),
  `LOST` (single edit, full loss).
- **`claude-time serve`** — HTMX-driven local server (`src/serve.rs`,
  `tiny_http`). Routes: `/`, `/window?since=X`, `/session/{id}`. Click a
  session row to expand per-file pinpoints inline.
- **Output modes**: `report` defaults to HTML + browser open; `--stdout`
  pipes HTML; `--markdown` keeps the legacy terminal-readable path;
  `--serve` starts the server.

### Test coverage

36 lib tests + 4 integration tests pass. New coverage in v0.1.1:
- `per_file_edits` extraction from transcripts
- `WastePinpoint::classify` across all 3 severities + ranking
- HTML render smoke tests (sections present, escaping correct)
- `serve` query-string + percent-decode helpers
- End-to-end: HTML default behavior, `--stdout`, `--markdown`

## Project layout

```
.
├── Cargo.toml                              # v0.1.1; metadata for crates.io
├── README.md                               # install routes + adoption section
├── LICENSE                                 # MIT
├── installer.sh                            # curl-shell installer
├── .claude-plugin/
│   ├── plugin.json                         # plugin manifest (v0.1.1)
│   └── marketplace.json                    # this repo IS its own marketplace
├── hooks/
│   └── hooks.json                          # SessionStart + SessionEnd declarations
├── skills/
│   └── claude-time-report/SKILL.md         # explains the HTML report on demand
├── src/
│   ├── main.rs                             # bin entry → cli::run
│   ├── lib.rs                              # module exports
│   ├── cli.rs                              # clap defs + dispatch
│   ├── paths.rs                            # CLAUDE_CONFIG_DIR-aware paths
│   ├── config.rs                           # TOML config
│   ├── model.rs                            # SessionRecord, Quadrant, WastePinpoint
│   ├── hooks.rs                            # capture from stdin JSON; fail-soft
│   ├── transcript.rs                       # JSONL streaming parser + per_file_edits
│   ├── git_history.rs                      # retention + score_per_file + pinpoint_waste
│   ├── report.rs                           # legacy markdown renderer
│   ├── html_report.rs                      # self-contained HTML + HTMX renderer
│   ├── serve.rs                            # tiny_http server, HTMX endpoints
│   ├── storage.rs                          # archive.jsonl compaction
│   └── install.rs                          # settings.json patcher
├── tests/
│   └── integration.rs                      # end-to-end via assert_cmd + tempfile
└── .github/
    ├── workflows/{ci,release}.yml
    └── ISSUE_TEMPLATE/{bug_report,feature_request}.md
```

## Critical context (don't break these)

- **Commit identity.** Author every commit as `Ayo M <ayodm@me.com>`. The
  gmail must not appear in this repo's history. `git config user.email`
  is set locally; verify before committing.

- **Storage rule.** Per-session JSON files are the hot path; `archive.jsonl`
  is the cold path. Block-overhead amplification on macOS APFS is ~16x —
  the rollup matters at scale. Don't reintroduce a many-small-files
  pattern for historical data.

- **Fail-soft hooks.** `src/hooks.rs::run_inner` errors are caught and
  logged to stderr — never propagated. A hook must NEVER crash a Claude
  Code session. Preserve this discipline.

- **No telemetry.** Adoption is tracked passively via crates.io + GitHub
  download counts + stars. Do not add HTTP egress to any third-party
  endpoint without an explicit opt-in flow.

- **`:schwab_order`-style non-retry policy.** This codebase doesn't have a
  Schwab order channel, but the spirit applies: when adding new external
  calls that could double-do something, never add retries by default.

- **Plugin install path.** Users do `/plugin marketplace add ayodm/claude-time`
  then `/plugin install claude-time@claude-time`. They still need the
  binary on `$PATH` (`cargo install claude-time` or installer.sh).

- **HTML is self-contained.** The rendered report works fine opened
  directly as a file. HTMX attrs are inert without a server but the
  static content is fully readable. Don't break this property.

## Running things locally

```sh
# Test everything (40 tests total)
cargo test

# Reinstall after changes
cargo install --path .

# Try the report
claude-time status
claude-time report                       # writes ~/.claude/claude-time/last-report.html, opens
claude-time report --since 30d --by project
claude-time report --serve               # HTMX server on :7878
claude-time report --markdown            # legacy

# Fake a session end-to-end:
echo '{"session_id":"test","cwd":"'$PWD'","transcript_path":"/tmp/x.jsonl"}' \
  | claude-time hook session-start
echo '{"session_id":"test","cwd":"'$PWD'"}' \
  | claude-time hook session-end
ls ~/.claude/claude-time/sessions/
```

## Cutting a release

```sh
git tag v0.1.1
git push origin v0.1.1
```

The release workflow handles the rest: cross-platform binaries (macOS
aarch64+x86_64, Linux x86_64+aarch64) attached to the GitHub Release with
sha256 sidecars, then `cargo publish` to crates.io (needs `CRATES_IO_TOKEN`
secret on the repo — set it via `gh secret set CRATES_IO_TOKEN` first).

## Follow-ups (not in v0.1.1)

- **Homebrew tap** — separate repo `ayodm/homebrew-claude-time` with
  formula pulling from GitHub Release. Do this after v0.1.1 is tagged so
  the release URL exists.
- **`claude-time inspect <session-id>`** — pretty-print one session.
- **Optional baseline-estimation slider** — opt-in `UserPromptSubmit` hook
  + 1-tap TUI prompt. Only if pinpoints prove too noisy.
- **zstd compression of `archive.jsonl`** — tier-3 storage win (~95%
  smaller). Adds `zstd` crate.
- **MCP server wrapper** — expose pinpoint queries as MCP tools so Claude
  can answer "where am I wasting time?" inline without the user running
  the command. Speculative — only if it clearly adds value beyond SKILL.md.

## Picking up where I left off

If you're a fresh Claude Code session opened here:

1. You just read this file.
2. `cargo test` to confirm 40 tests still pass.
3. Check the open follow-ups list above. Most-likely next moves:
   either tag the release, set up the Homebrew tap, or implement
   `claude-time inspect`.
4. Commit per logical chunk with the `ayodm@me.com` identity.

Welcome back.
