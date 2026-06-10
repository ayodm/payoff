use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "payoff",
    version,
    about = "Passive ROI tracker for AI coding sessions. Did the session pay off?",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Patch ~/.claude/settings.json with SessionStart + SessionEnd hooks.
    Install,
    /// Show installation state and how many sessions are captured.
    Status,
    /// Hook entry point — Claude Code invokes this with stdin JSON.
    Hook {
        /// Which hook event fired.
        #[arg(value_enum)]
        event: HookEvent,
    },
    /// Build the report. Default: writes self-contained HTML and opens browser.
    Report {
        /// Time window (e.g. `7d`, `30d`, `24h`).
        #[arg(long, default_value = "7d")]
        since: String,
        /// Override the retention window (days).
        #[arg(long)]
        retention_window: Option<u32>,
        /// Group results by `project`.
        #[arg(long)]
        by: Option<String>,
        /// Write HTML to this path instead of the default `<data_dir>/last-report.html`.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Pipe HTML to stdout instead of writing a file. Best for CI / further processing.
        #[arg(long, conflicts_with_all = ["out", "markdown", "serve"])]
        stdout: bool,
        /// Legacy markdown output (terminal-readable). Goes to stdout.
        #[arg(long, conflicts_with_all = ["out", "stdout", "serve"])]
        markdown: bool,
        /// Start an HTMX-driven local server instead of writing a file.
        #[arg(long, conflicts_with_all = ["out", "stdout", "markdown"])]
        serve: bool,
        /// Port for `--serve`.
        #[arg(long, default_value_t = 7878)]
        port: u16,
        /// Suppress the automatic browser open (default HTML mode).
        #[arg(long)]
        no_open: bool,
    },
    /// Start the HTMX server explicitly. Equivalent to `report --serve`.
    Serve {
        #[arg(long, default_value_t = 7878)]
        port: u16,
    },
    /// Remove our hooks from settings.json. Leaves the data directory alone.
    Uninstall,
    /// Roll closed sessions older than the retention window into archive.jsonl.
    Archive,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum HookEvent {
    SessionStart,
    SessionEnd,
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Install => crate::install::install(),
        Command::Status => crate::install::status(),
        Command::Hook { event } => crate::hooks::run(event),
        Command::Report {
            since,
            retention_window,
            by,
            out,
            stdout,
            markdown,
            serve,
            port,
            no_open,
        } => {
            if serve {
                return crate::serve::run(port);
            }
            if markdown {
                return crate::report::run(&since, retention_window, by.as_deref());
            }
            run_html_report(
                &since,
                retention_window,
                by.as_deref(),
                out,
                stdout,
                no_open,
            )
        }
        Command::Serve { port } => crate::serve::run(port),
        Command::Uninstall => crate::install::uninstall(),
        Command::Archive => {
            let cfg = crate::config::load()?;
            let cutoff = crate::storage::default_archive_cutoff(cfg.report.retention_window_days);
            let n = crate::storage::archive_older_than(cutoff)?;
            println!(
                "Archived {n} session(s) older than {} day(s).",
                cfg.report.retention_window_days + 1
            );
            Ok(())
        }
    }
}

fn run_html_report(
    since: &str,
    retention_window: Option<u32>,
    by: Option<&str>,
    out: Option<PathBuf>,
    stdout: bool,
    no_open: bool,
) -> Result<()> {
    let cfg = crate::config::load().unwrap_or_default();
    let window_days = retention_window.unwrap_or(cfg.report.retention_window_days);

    // Opportunistic compaction — same as markdown path.
    let archive_cutoff = crate::storage::default_archive_cutoff(window_days);
    let _ = crate::storage::archive_older_than(archive_cutoff);

    let cutoff = crate::report::parse_since(since)?;
    let mut sessions = crate::report::load_sessions_since(cutoff)?;
    sessions.extend(crate::storage::load_archive_since(cutoff)?);

    let html = crate::html_report::render(&sessions, &cfg, by);

    if stdout {
        print!("{html}");
        return Ok(());
    }

    let dest = match out {
        Some(p) => p,
        None => {
            crate::paths::ensure_dirs()?;
            crate::paths::data_dir()?.join("last-report.html")
        }
    };
    std::fs::write(&dest, &html).with_context(|| format!("writing {}", dest.display()))?;
    println!("Wrote {} ({} bytes).", dest.display(), html.len());

    if !no_open {
        if let Err(err) = open::that_detached(&dest) {
            eprintln!("[payoff] could not auto-open browser: {err}");
        }
    }
    Ok(())
}
