//! Hang warning after a short CLI command returns but the process stays up.
//!
//! MCP children, the workspace index watcher, and leftover HTTP clients can
//! keep Tokio alive after `debug` / `config` / `session list` / `generate` have printed.
//! Long-running commands (`serve`, TUI `run`) skip this.

use std::time::{Duration, Instant};

#[cfg(not(test))]
const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);
#[cfg(test)]
const SHUTDOWN_WAIT: Duration = Duration::from_millis(80);

pub(crate) fn is_short_command(cli: &crate::Cli) -> bool {
    use crate::Commands;
    match &cli.command {
        None => false,
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
            | Commands::Completions { .. }
            | Commands::Generate { .. } => true,
            Commands::Run { .. }
            | Commands::Acp
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
mod tests {
    use super::*;
    use crate::{Cli, Commands};
    use clap::Parser;

    fn cli(command: Option<Commands>) -> Cli {
        let mut parsed = Cli::try_parse_from(["whycodes"]).unwrap();
        parsed.command = command;
        parsed
    }

    #[test]
    fn short_commands_are_debug_config_session() {
        assert!(is_short_command(&cli(Some(Commands::Debug {
            json: false
        }))));
        assert!(is_short_command(&cli(Some(Commands::Stats))));
        assert!(is_short_command(&cli(Some(Commands::Config {
            cmd: crate::ConfigCmd::Path,
        }))));
        assert!(is_short_command(&cli(Some(Commands::Provider {
            cmd: crate::ProviderCmd::List,
        }))));
        assert!(is_short_command(&cli(Some(Commands::Model {
            cmd: crate::ModelCmd::List,
        }))));
        assert!(is_short_command(&cli(Some(Commands::Agent { name: None }))));
        assert!(is_short_command(&cli(Some(Commands::Plugins {
            cmd: None
        }))));
        assert!(is_short_command(&cli(Some(Commands::Session {
            cmd: crate::SessionCmd::List,
        }))));
        assert!(is_short_command(&cli(Some(Commands::Memory {
            cmd: crate::MemoryCmd::Path,
        }))));
        assert!(is_short_command(&cli(Some(Commands::Import {
            args: crate::ImportArgs {
                from: None,
                dry_run: true,
                yes: true,
                force: false,
            }
        }))));
        assert!(is_short_command(&cli(Some(Commands::Completions {
            shell: clap_complete::Shell::Bash,
        }))));
        assert!(!is_short_command(&cli(None)));
        assert!(!is_short_command(&cli(Some(Commands::Run {
            prompt: None,
            max_turns: None,
            format: whycodes_protocol::OutputFormat::Text,
        }))));
        assert!(is_short_command(&cli(Some(Commands::Generate {
            prompt: vec!["x".into()],
            max_turns: None,
            jobs: 1,
            format: whycodes_protocol::OutputFormat::Text,
        }))));
        assert!(!is_short_command(&cli(Some(Commands::Acp))));
        assert!(!is_short_command(&cli(Some(Commands::Pr {
            title: None,
            base: None,
        }))));
        assert!(!is_short_command(&cli(Some(Commands::Github {
            cmd: crate::GithubCmd::Pr { action: None },
        }))));
        assert!(!is_short_command(&cli(Some(Commands::Web))));
        assert!(!is_short_command(&cli(Some(Commands::Mcp {
            cmd: crate::McpCmd::List,
        }))));
        assert!(!is_short_command(&cli(Some(Commands::Connect {
            addr: "127.0.0.1:1".into(),
            session: None,
        }))));
        assert!(!is_short_command(&cli(Some(Commands::Auth {
            cmd: crate::AuthCmd::Status,
        }))));
        #[cfg(feature = "server")]
        assert!(!is_short_command(&cli(Some(Commands::Serve {
            port: 3030,
            no_takeover: false,
        }))));
        #[cfg(feature = "self-update")]
        assert!(!is_short_command(&cli(Some(Commands::Upgrade))));
    }

    #[test]
    fn hang_message_is_one_line() {
        let msg = hang_message();
        assert!(msg.contains("5s"), "{msg}");
        assert!(!msg.contains('\n'), "{msg}");
    }

    #[test]
    fn shutdown_runtime_returns_immediately_when_idle() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        shutdown_runtime(rt);
    }

    #[test]
    fn shutdown_runtime_warns_when_work_outlives_budget() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.spawn(async {
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        shutdown_runtime(rt);
    }
}
