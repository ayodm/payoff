---
name: payoff-setup
description: Use when the user wants to install, uninstall, configure, or troubleshoot the payoff tracker — "set up payoff", "is payoff installed?", "payoff isn't capturing my sessions", "/doctor flags hook errors", "change my hourly rate", "where does payoff store data?". Covers the binary-on-PATH requirement, the wrapped-hook settings.json shape, legacy migration, and config.toml.
---

# payoff-setup

Owns the install / health / troubleshoot lifecycle. For reading a report use
`payoff-report`; for diagnosing wasted edits use `payoff-waste-triage`.

## When this skill fires

- "install payoff" / "set up the tracker"
- "is payoff installed?" / "is it capturing sessions?"
- "payoff isn't recording anything" / "no sessions show up"
- "/doctor is complaining about hooks"
- "uninstall payoff" / "remove the hooks"
- "change my hourly rate" / "set the retention window" / "exclude a directory"

## Two-part install (both required)

payoff is a binary **and** a pair of hooks. The plugin ships the hooks; the
binary must be on `$PATH` separately.

1. **Binary** — one of:
   ```sh
   cargo install payoff                       # Rust users
   # or the curl installer (see README) for non-Rust users
   ```
2. **Hooks** — either:
   ```sh
   payoff install                             # patches ~/.claude/settings.json
   ```
   or via the plugin:
   ```
   /plugin marketplace add ayodm/payoff
   /plugin install payoff@payoff
   ```

Confirm both with:

```sh
payoff status
```

Healthy output shows `hooks installed: 2 / 2` and a `sessions dir` path. If
`hooks installed: 0 / 2`, run `payoff install`. If the command isn't found,
the binary isn't on `$PATH` — fix step 1.

## Troubleshooting

**"`payoff` not found"** — binary isn't installed or not on `$PATH`. See step 1.
The plugin alone does NOT install the binary.

**`/doctor` says `Expected array, but received undefined`** — legacy v0.1.x
wrote a flat `{type, command}` hook entry; Claude Code now requires the
wrapped `{hooks: [{type, command}]}` shape. Fix is idempotent:

```sh
payoff install      # self-heals: rewrites flat entries to wrapped, no dupes
```

**Sessions dir empty after working a while** — hooks aren't firing. Check
`payoff status`; if `2 / 2` but still empty, confirm the hook command in
`~/.claude/settings.json` points at the same binary `which payoff` resolves
to (a stale install elsewhere on `$PATH` is the usual culprit).

**Want to back out** — `payoff uninstall` removes our hooks (flat or wrapped)
and leaves the data dir alone. A pre-install backup may exist at
`~/.claude/settings.json.before-payoff.bak`.

## Configuration

Edit `~/.claude/payoff/config.toml` (created on first edit):

```toml
[report]
retention_window_days = 7      # days before a session's retention is scored
hourly_rate_usd       = 0.0    # 0.0 = report only Claude $ cost; set to include your time
default_period        = "7d"   # default --since when the CLI omits it

[exclude]
# Path fragments skipped when counting session lines (generated output).
paths = [".git", "node_modules", "target", "dist", ".next", "_build", "deps"]
```

Add a build/output directory to `exclude.paths` if generated files are
inflating line counts and skewing retention.

## Where things live

- Per-session records: `~/.claude/payoff/sessions/*.json` (hot path)
- Compacted history: `~/.claude/payoff/archive.jsonl` (run `payoff archive`
  to roll closed sessions older than the retention window into it)
- Last rendered report: `~/.claude/payoff/last-report.html`
- Config: `~/.claude/payoff/config.toml`
- Raw transcripts (read by waste-triage): `~/.claude/projects/<project>/<session-id>.jsonl`
