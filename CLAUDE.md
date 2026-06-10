# payoff — current state

This file auto-loads when a Claude Code session starts in this directory.
Read it first.

---

## What this project is

A passive ROI tracker for Claude Code itself. Measures whether Claude is
*saving you time* by substituting **diff retention** for the unknowable
"time saved" baseline. A session's diff that still exists in HEAD after N
days = value; a diff that was reverted or rewritten = waste.

- **Repo:** https://github.com/ayodm/payoff (public)
- **Latest:** v0.2.0 on `main`. The crate's earlier v0.1.x and
  v0.2.0-rc.x lines stay on crates.io as tombstones (yanked); new
  releases go out as `payoff`.
- **Stack:** Rust (single binary, no runtime deps for users)
- **Distribution:** crates.io + GitHub Releases + Claude Code plugin marketplace + Homebrew tap (planned)
- **License:** MIT
- **Commit identity for this repo:** `Ayo M <ayodm@me.com>`. The gmail
  address must NOT appear in commits. Local git config is set; verify with
  `git config user.email`.
- **Installed on this machine:** `~/.cargo/bin/payoff` (v0.2.0
  built locally) with hooks active in `~/.claude/settings.json`. Backup
  of pre-install settings at `~/.claude/settings.json.before-payoff.bak`.

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
- **`payoff serve`** — HTMX-driven local server (`src/serve.rs`,
  `tiny_http`). Routes: `/`, `/window?since=X`, `/session/{id}`. Click a
  session row to expand per-file pinpoints inline.
- **Output modes**: `report` defaults to HTML + browser open; `--stdout`
  pipes HTML; `--markdown` keeps the legacy terminal-readable path;
  `--serve` starts the server.

### v0.2.0 — driver capture + correlation

Pre-release. Stable line stays on v0.1.2; this is opt-in via
`cargo install payoff`. Plan source-of-truth at
`docs/v0.2.0-plan.md`; per-phase commits:

- **Phase 0** (`eed3cbf`) — surface cache hit ratio + sessions-by-model
  in HTML/markdown totals and the `/session/{id}` HTMX fragment. New
  `SessionRecord::cache_hit_ratio() -> Option<f64>`.
- **Phase 1** (`b40cc55`) — new `src/env_capture.rs` snapshots
  active skills (user / project / enabled-plugin sources), CLAUDE.md
  file hashes (walked up to repo root, depth-cap 8), active hook
  events (excluding our own `payoff hook *`), and enabledPlugins
  at SessionStart. Unknown payload keys collected into an `env_extras`
  bag for forward compat. Content hashing via inline FNV-1a 64-bit
  (no new dep).
- **Phase 2** (`67e3173`) — new `src/correlate.rs`. `group_by_driver`
  partitions sessions by skill / CLAUDE.md hash / hook event / model /
  edit-pattern, computes per-group avg retention + cost, filters
  `n < 3` as noise. Reports gain a "Drivers" section between
  pinpoints and quadrant (HTML + markdown). HTMX endpoint
  `GET /driver/{type}/{key}` returns a session-list fragment for
  click-to-drill-in. Tool-call ordering captured into
  `SessionRecord::tool_sequence` (cap 200) for the
  Edit-without-prior-Read detector and future 3-gram mining.

Phases 3 (`UserPromptSubmit` hook for prompt-shape) and 4 (cache
buckets + tool-sequence 3-gram mining) are deferred — see plan.

### v0.1.2 — hook shape fix + legacy migration

- **Hook entries now wrapped correctly.** v0.1.x wrote
  `{type, command}` directly under `hooks.SessionStart`/`SessionEnd`,
  which Claude Code's settings validator now rejects (`/doctor` flags
  `Expected array, but received undefined`) and which also rendered
  the hooks silently inert. v0.1.2 writes the wrapped shape
  `{hooks: [{type, command}]}` Claude Code expects
  (`src/install.rs::wrapped_entry`).
- **Self-healing install.** Running `payoff install` against a
  settings.json with legacy flat entries rewrites them in place. No
  duplicates, even from half-migrated states or accidental flat dups
  — `apply_hooks` collapses every configuration toward "exactly one
  wrapped entry per event".
- **Two-shape uninstall.** Accepts both flat and wrapped entries so
  users on either version remove cleanly.
- **Plugin manifest** (`hooks/hooks.json`) updated to wrapped shape so
  the plugin install path is correct out of the box.

### Test coverage

44 lib tests + 5 integration tests pass. New coverage in v0.1.2
(`src/install.rs::tests`, `tests/integration.rs`):
- Shape assertion on emitted entries
- Migration of legacy flat → wrapped, preserving siblings
- Dedupe of half-migrated and duplicate-flat states
- Uninstall removes wrapped, flat, and mixed-shape entries
- `count_our_hooks` recognises both shapes
- End-to-end integration test: seed settings.json with legacy shape,
  run `payoff install`, assert wrapped output

Earlier v0.1.1 coverage (still present):
- `per_file_edits` extraction from transcripts
- `WastePinpoint::classify` across all 3 severities + ranking
- HTML render smoke tests (sections present, escaping correct)
- `serve` query-string + percent-decode helpers
- End-to-end: HTML default behavior, `--stdout`, `--markdown`

## Project layout

```
.
├── Cargo.toml                              # v0.2.0; metadata for crates.io
├── README.md                               # install routes + adoption section
├── LICENSE                                 # MIT
├── installer.sh                            # curl-shell installer
├── .claude-plugin/
│   ├── plugin.json                         # plugin manifest (v0.2.0)
│   └── marketplace.json                    # this repo IS its own marketplace
├── hooks/
│   └── hooks.json                          # SessionStart + SessionEnd declarations
├── skills/
│   └── payoff-report/SKILL.md         # explains the HTML report on demand
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

- **Plugin install path.** Users do `/plugin marketplace add ayodm/payoff`
  then `/plugin install payoff@payoff`. They still need the
  binary on `$PATH` (`cargo install payoff` or installer.sh).

- **HTML is self-contained.** The rendered report works fine opened
  directly as a file. HTMX attrs are inert without a server but the
  static content is fully readable. Don't break this property.

## Running things locally

```sh
# Test everything (49 tests total: 44 lib + 5 integration)
cargo test

# Reinstall after changes
cargo install --path .

# Try the report
payoff status
payoff report                       # writes ~/.claude/payoff/last-report.html, opens
payoff report --since 30d --by project
payoff report --serve               # HTMX server on :7878
payoff report --markdown            # legacy

# Fake a session end-to-end:
echo '{"session_id":"test","cwd":"'$PWD'","transcript_path":"/tmp/x.jsonl"}' \
  | payoff hook session-start
echo '{"session_id":"test","cwd":"'$PWD'"}' \
  | payoff hook session-end
ls ~/.claude/payoff/sessions/
```

## Cutting a release

```sh
git tag v0.2.0
git push origin v0.2.0
```

The release workflow handles the rest: cross-platform binaries (macOS
aarch64+x86_64, Linux x86_64+aarch64) attached to the GitHub Release with
sha256 sidecars, then `cargo publish` to crates.io (needs `CRATES_IO_TOKEN`
secret on the repo). Pre-release tags (`v*-rc.*`, `v*-beta.*`) are still
picked up by the workflow; after release, mark the GitHub Release as
pre-release with `gh release edit <tag> --prerelease` so users see the
status flag. crates.io respects semver pre-release identifiers — bare
`cargo install payoff` keeps users on the latest stable.

## Follow-ups (not in v0.2.0)

- **Live-session validation — DONE.** The SessionStart/SessionEnd hooks
  are confirmed firing from inside real Claude Code sessions;
  `~/.claude/payoff/sessions/*.json` populates with real `model`,
  `claude_md_files`, and `enabled_plugins`. Plugin-skill discovery in
  `env_capture.rs` was fixed to walk the `plugins/cache/<mp>/<plugin>/
  <version>/skills/` layout (the old `marketplaces/` path matched
  nothing) and verified to capture a real plugin skill. The
  "experimental" disclaimer is retired. Two fields stay environment-
  dependent (not defects): `active_skills` is empty unless a
  skill-bearing plugin is enabled or a `~/.claude/skills/` entry exists,
  and `active_hook_events` is empty when the only `settings.json` hooks
  are payoff's own (self-excluded).
- **Phase 3 — `UserPromptSubmit` hook** for prompt-shape capture (length,
  has-code-block, question vs command). Higher risk: third hook event,
  install migration impact, Claude Code payload key unverified. See plan.
- **Phase 4 — cache buckets + tool-sequence 3-gram mining.** Builds on
  Phase 2's correlation infrastructure.
- **Homebrew tap** — separate repo `ayodm/homebrew-payoff` with
  formula pulling from GitHub Release. Do this after v0.2.0 stable is
  tagged so the release URL stabilises.
- **`payoff inspect <session-id>`** — pretty-print one session.
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
2. `cargo test` to confirm 107 tests still pass.
3. Check the open follow-ups list above. Most-likely next moves:
   validate v0.2.0 in a live Claude Code session (open the app,
   work normally, then `payoff report --serve` and confirm the
   Drivers section shows non-empty groups with the right environment
   features). After that, decide whether to promote to v0.2.0 stable or
   start Phase 3.
4. Commit per logical chunk with the `ayodm@me.com` identity.

Welcome back.
