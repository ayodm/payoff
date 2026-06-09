use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "claude-time",
    version,
    about = "Passive-only ROI tracker for Claude Code sessions",
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
    /// Render a markdown report to stdout.
    Report {
        /// Time window for the report (e.g. `7d`, `30d`, `24h`).
        #[arg(long, default_value = "7d")]
        since: String,
        /// Override the retention window (days).
        #[arg(long)]
        retention_window: Option<u32>,
        /// Group results by `project` or none.
        #[arg(long)]
        by: Option<String>,
    },
    /// Remove our hooks from settings.json. Leaves the data directory alone.
    Uninstall,
    /// Roll closed sessions older than the retention window into archive.jsonl
    /// (eliminates per-file block overhead). The report command runs this
    /// implicitly; use it manually if you want immediate compaction.
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
        } => crate::report::run(&since, retention_window, by.as_deref()),
        Command::Uninstall => crate::install::uninstall(),
        Command::Archive => {
            let cfg = crate::config::load()?;
            let cutoff =
                crate::storage::default_archive_cutoff(cfg.report.retention_window_days);
            let n = crate::storage::archive_older_than(cutoff)?;
            println!("Archived {n} session(s) older than {} day(s).",
                cfg.report.retention_window_days + 1);
            Ok(())
        }
    }
}
