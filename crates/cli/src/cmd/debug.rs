//! Debug dump and post-TUI exit.
use super::helpers::*;
use crate::{Cli, Commands, PKG_VERSION, VERSION_LONG};
use colored::*;
use whycodes_config::Config;
use whycodes_protocol::OutputFormat;

pub(crate) async fn cmd_debug() -> anyhow::Result<()> {
    println!("{} Debug Information:", "🔧".bold());
    println!("  Version:     {}", VERSION_LONG.cyan());

    // Config path
    match Config::default_path() {
        Ok(p) => {
            let exists = if p.exists() {
                "✓".green()
            } else {
                "✗ (not found)".red()
            };
            println!("  Config:      {} {}", p.display(), exists);
        }
        Err(e) => {
            println!("  Config:      error: {}", e);
        }
    }

    // Data directory + log paths (Grok-style)
    match Config::data_dir() {
        Ok(p) => {
            let exists = if p.exists() {
                "✓".green()
            } else {
                "✗".red()
            };
            println!("  Data dir:    {} {}", p.display(), exists);
            let dirs = whycodes_core::logging::LogDirs::from_data_dir(&p);
            println!(
                "  JSONL log:   {} {}",
                dirs.unified_jsonl().display(),
                if dirs.unified_jsonl().exists() {
                    "✓".green()
                } else {
                    "·".dimmed()
                }
            );
            println!("  Crash dir:   {}", dirs.crash.display());
            println!(
                "  Debug log:   {} (or WHYCODES_LOG_FILE / --debug)",
                dirs.debug.join("latest.log").display()
            );
        }
        Err(e) => {
            println!("  Data dir:    error: {}", e);
        }
    }

    // Current directory
    match std::env::current_dir() {
        Ok(p) => println!("  CWD:         {}", p.display()),
        Err(e) => println!("  CWD:         error: {}", e),
    }

    // Home directory
    if let Ok(home) = std::env::var("HOME") {
        println!("  HOME:        {}", home);
    }

    // Rust toolchain
    if let Ok(rustc) = std::process::Command::new("rustc")
        .arg("--version")
        .output()
    {
        let ver = String::from_utf8_lossy(&rustc.stdout).trim().to_string();
        println!("  Rust:        {}", ver);
    }

    // Git info
    if let Ok(git) = std::process::Command::new("git").arg("--version").output() {
        let ver = String::from_utf8_lossy(&git.stdout).trim().to_string();
        println!("  Git:         {}", ver);
    }

    // Relevant environment variables
    println!("  Environment:");
    for var in &[
        "WHYCODES_PROVIDER",
        "WHYCODES_MODEL",
        "WHYCODES_LOG_LEVEL",
        "WHYCODES_LOG_FILE",
        "RUST_LOG",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "DEEPSEEK_API_KEY",
        "GOOGLE_API_KEY",
        "XAI_API_KEY",
    ] {
        match std::env::var(var) {
            Ok(val) => {
                println!("    {} = {} (set)", var, mask_secret(&val).dimmed());
            }
            Err(_unset) => {
                println!("    {} = (not set)", var.dimmed());
            }
        }
    }

    // OAuth subscription logins — method + expiry only, never token material.
    println!("  OAuth (auth.json):");
    match Config::data_dir() {
        Ok(dir) => {
            let store = whycodes_auth::TokenStore::new(&dir);
            match store.list() {
                Ok(entries) if entries.is_empty() => {
                    println!("    (none — `whycodes auth login <provider>`)");
                }
                Ok(entries) => {
                    for (name, auth) in entries {
                        println!(
                            "    {:<15} {} · {}",
                            name,
                            auth.method,
                            super::auth::auth_expiry_label(&auth)
                        );
                    }
                }
                Err(e) => println!("    error reading store: {e}"),
            }
        }
        Err(e) => println!("    data dir error: {e}"),
    }

    Ok(())
}

/// Ask GitHub for a newer tag in the background. The TUI paints first; if a
/// release exists, the home screen offers a confirm (never a silent replace).
pub(crate) fn spawn_update_check(
    cli: &Cli,
    config: &Config,
) -> Option<tokio::sync::mpsc::UnboundedReceiver<whycodes_tui::UpdateOffer>> {
    if !should_auto_update(cli, config.general.auto_update) {
        return None;
    }
    #[cfg(feature = "self-update")]
    {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            match crate::upgrade::check_latest().await {
                Ok(Some(rel)) => {
                    let offer = if rel.homebrew {
                        whycodes_tui::UpdateOffer::Homebrew(rel.version)
                    } else {
                        whycodes_tui::UpdateOffer::SelfInstall(rel.version)
                    };
                    if tx.send(offer).is_err() {
                        tracing::debug!("update offer dropped: tui already quit");
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::debug!("startup update check skipped: {e}");
                }
            }
        });
        Some(rx)
    }
    #[cfg(not(feature = "self-update"))]
    {
        let _ = cli;
        let _ = config;
        None
    }
}

pub(crate) async fn after_tui_exit(exit: whycodes_tui::TuiExit) -> anyhow::Result<()> {
    match exit {
        whycodes_tui::TuiExit::Quit => Ok(()),
        whycodes_tui::TuiExit::Upgrade => {
            #[cfg(feature = "self-update")]
            {
                match crate::upgrade::run().await {
                    Ok(Some(version)) => {
                        eprintln!(
                            "whycodes: updated {} → {version} — restart to use it",
                            PKG_VERSION
                        );
                    }
                    Ok(None) => {
                        eprintln!("whycodes: already on the latest release");
                    }
                    Err(e) => {
                        eprintln!("whycodes: update failed: {e}");
                    }
                }
            }
            Ok(())
        }
    }
}

pub(crate) fn should_auto_update(cli: &Cli, config_enabled: bool) -> bool {
    should_auto_update_with_env(
        cli,
        config_enabled,
        std::env::var_os("WHYCODES_NO_AUTO_UPDATE").is_some(),
        std::env::var_os("CI").is_some(),
    )
}

/// Same gates as [`should_auto_update`], with env flags passed in so unit
/// tests stay deterministic under GitHub Actions (`CI=true`).
pub(crate) fn should_auto_update_with_env(
    cli: &Cli,
    config_enabled: bool,
    no_auto_update_env: bool,
    ci_env: bool,
) -> bool {
    if cli.no_auto_update || !config_enabled || no_auto_update_env || ci_env {
        return false;
    }
    match &cli.command {
        None => true,
        Some(Commands::Run { format, .. }) => matches!(format, OutputFormat::Text),
        Some(_) => false,
    }
}
