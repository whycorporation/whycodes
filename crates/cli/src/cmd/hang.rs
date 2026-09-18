//! Hang warning after a command returns but the process stays up.
//!
//! MCP children, the workspace index watcher, leftover HTTP clients, and an
//! in-flight LLM stream can keep Tokio alive after the TUI has already printed
//! the session summary. Short commands (`debug` / `config` / `session list`)
//! and interactive TUI `run` both need a bounded drop. `serve` skips this.

use std::time::{Duration, Instant};

#[cfg(not(test))]
const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);
#[cfg(test)]
const SHUTDOWN_WAIT: Duration = Duration::from_millis(80);

pub(crate) fn is_short_command(cli: &crate::Cli) -> bool {
    use crate::Commands;
    match &cli.command {
        None => true, // interactive TUI (`whycodes` / extra flags)
        Some(cmd) => match cmd {
            Commands::Provider { .. }
            | Commands::Model { .. }
            | Commands::Agent { .. }
            | Commands::Plugins { .. }
            | Commands::Config { .. }
            | Commands::Session { .. }
            | Commands::Memory { .. }
            | Commands::Import { .. }
            | Commands::Stats
            | Commands::Debug { .. }
            | Commands::Slop { .. }
            | Commands::Completions { .. }
            | Commands::Generate { .. }
            | Commands::Run { .. } => true,
            Commands::Acp
            | Commands::Pr { .. }
            | Commands::Github { .. }
            | Commands::Web
            | Commands::Mcp { .. }
            | Commands::Connect { .. }
            | Commands::Auth { .. } => false,
            #[cfg(feature = "server")]
            Commands::Serve { .. } => false,
            #[cfg(feature = "self-update")]
            Commands::Upgrade => false,
        },
    }
}

/// Drop the runtime, waiting at most [`SHUTDOWN_WAIT`]. If work is still
/// outstanding after that, print one diagnostic line. Does not keep the
/// process alive on a clean exit (`shutdown_timeout` returns immediately
/// when the queue is empty).
pub(crate) fn shutdown_runtime(rt: tokio::runtime::Runtime) {
    let start = Instant::now();
    rt.shutdown_timeout(SHUTDOWN_WAIT);
    if start.elapsed() >= SHUTDOWN_WAIT {
        eprintln!("{}", hang_message());
    }
}

pub(crate) fn hang_message() -> String {
    "whycodes finished but the process is still shutting down after 5s \
     (background task, child process, or file watcher)."
        .into()
}

#[cfg(test)]
#[path = "hang_tests.rs"]
mod tests;
