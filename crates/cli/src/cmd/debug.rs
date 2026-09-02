//! Debug dump and post-TUI exit.
use super::helpers::*;
use crate::{Cli, Commands, PKG_VERSION, VERSION_LONG};
use colored::*;
use serde::Serialize;
use whycodes_config::Config;
use whycodes_protocol::OutputFormat;

const DEBUG_ENV_VARS: &[&str] = &[
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
];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DebugDump {
    version: String,
    git_hash: String,
    config_path: Option<String>,
    config_exists: bool,
    data_dir: Option<String>,
    jsonl_log: Option<String>,
    crash_dir: Option<String>,
    debug_log: Option<String>,
    cwd: Option<String>,
    home: Option<String>,
    rustc: Option<String>,
    git: Option<String>,
    env: Vec<DebugEnv>,
    oauth: Vec<DebugOauth>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DebugEnv {
    name: String,
    set: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DebugOauth {
    provider: String,
    method: String,
    expiry: String,
}

fn collect_debug() -> DebugDump {
    let config_path = match Config::default_path() {
        Ok(p) => Some(p),
        Err(err) => {
            tracing::debug!(error = %err, "debug dump: config path unavailable");
            None
        }
    };
    let config_exists = config_path.as_ref().is_some_and(|p| p.exists());
    let data_dir = match Config::data_dir() {
        Ok(p) => Some(p),
        Err(err) => {
            tracing::debug!(error = %err, "debug dump: data dir unavailable");
            None
        }
    };
    let (jsonl_log, crash_dir, debug_log) = match &data_dir {
        Some(p) => {
            let dirs = whycodes_core::logging::LogDirs::from_data_dir(p);
            (
                Some(dirs.unified_jsonl().display().to_string()),
                Some(dirs.crash.display().to_string()),
                Some(dirs.debug.join("latest.log").display().to_string()),
            )
        }
        None => (None, None, None),
    };
    let env = DEBUG_ENV_VARS
        .iter()
        .map(|name| DebugEnv {
            name: (*name).to_string(),
            set: std::env::var(name).is_ok(),
        })
        .collect();
    let oauth = match &data_dir {
        Some(dir) => match whycodes_auth::TokenStore::new(dir).list() {
            Ok(entries) => entries
                .into_iter()
                .map(|(provider, auth)| DebugOauth {
                    provider,
                    method: auth.method.clone(),
                    expiry: super::auth::auth_expiry_label(&auth),
                })
                .collect(),
            Err(err) => {
                tracing::debug!(error = %err, "debug dump: oauth store unread");
                Vec::new()
            }
        },
        None => Vec::new(),
    };
    DebugDump {
        version: PKG_VERSION.to_string(),
        git_hash: env!("WHYCODES_GIT_HASH").to_string(),
        config_path: config_path.map(|p| p.display().to_string()),
        config_exists,
        data_dir: data_dir.map(|p| p.display().to_string()),
        jsonl_log,
        crash_dir,
        debug_log,
        cwd: std::env::current_dir()
            .ok()
            .map(|p| p.display().to_string()),
        home: std::env::var("HOME").ok(),
        rustc: cmd_version("rustc"),
        git: cmd_version("git"),
        env,
        oauth,
    }
}

fn cmd_version(bin: &str) -> Option<String> {
    std::process::Command::new(bin)
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

pub(crate) async fn cmd_debug(json: bool) -> anyhow::Result<()> {
    if json {
        let dump = collect_debug();
        println!("{}", serde_json::to_string_pretty(&dump)?);
        return Ok(());
    }

    println!("{} Debug Information:", "🔍".bold());
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

    // Relevant environment variables. Secrets are masked; `--json` only
    // reports whether the name is set (never the value).
    println!("  Environment:");
    for var in DEBUG_ENV_VARS {
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
            if std::env::var_os("WHYCODES_TEST_SKIP_UPGRADE").is_some() {
                return;
            }
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
                if std::env::var_os("WHYCODES_TEST_SKIP_UPGRADE").is_some() {
                    eprintln!("whycodes: already on the latest release");
                    return Ok(());
                }
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
        std::env::var_os("WHYCODES_BENCH").is_some_and(|v| !v.is_empty()),
    )
}

/// Same gates as [`should_auto_update`], with env flags passed in so unit
/// tests stay deterministic under GitHub Actions (`CI=true`) and first-frame
/// harness runs (`WHYCODES_BENCH`).
pub(crate) fn should_auto_update_with_env(
    cli: &Cli,
    config_enabled: bool,
    no_auto_update_env: bool,
    ci_env: bool,
    bench_env: bool,
) -> bool {
    if cli.no_auto_update || !config_enabled || no_auto_update_env || ci_env || bench_env {
        return false;
    }
    match &cli.command {
        None => true,
        Some(Commands::Run { format, .. }) => matches!(format, OutputFormat::Text),
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::Cli;
    use clap::Parser;

    #[test]
    fn auto_update_off_when_cli_flag() {
        let cli = Cli::try_parse_from(["whycodes", "--no-auto-update"]).unwrap();
        assert!(!should_auto_update_with_env(
            &cli, true, false, false, false
        ));
    }

    #[test]
    fn debug_dump_keys_are_camel_case() {
        let dump = collect_debug();
        let v = serde_json::to_value(&dump).unwrap();
        for key in [
            "version",
            "gitHash",
            "configPath",
            "configExists",
            "dataDir",
            "jsonlLog",
            "crashDir",
            "debugLog",
            "cwd",
            "env",
            "oauth",
        ] {
            assert!(v.get(key).is_some(), "missing {key} in {v}");
        }
        assert!(v.get("git_hash").is_none());
        let env = v["env"].as_array().expect("env array");
        assert!(!env.is_empty());
        for entry in env {
            assert!(entry.get("name").is_some());
            assert!(entry.get("set").and_then(|s| s.as_bool()).is_some());
            assert!(entry.get("value").is_none(), "env must not leak values");
        }
    }
}
