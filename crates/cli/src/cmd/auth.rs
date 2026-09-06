//! Auth login/logout/import.
use super::helpers::*;
use crate::args::*;
use colored::*;
use whycodes_config::Config;

pub(crate) async fn cmd_auth(cmd: &AuthCmd) -> anyhow::Result<()> {
    let data_dir = Config::data_dir()?;
    let store = whycodes_auth::TokenStore::new(&data_dir);
    match cmd {
        AuthCmd::Login {
            provider,
            no_browser,
        } => {
            let test_login = std::env::var_os("WHYCODES_TEST_AUTH_LOGIN").is_some();
            if !test_login && !whycodes_auth::providers::supports_oauth(provider) {
                anyhow::bail!(
                    "provider `{provider}` does not support OAuth login (supported: {})",
                    oauth_provider_list()
                );
            }
            // Unit tests skip the browser/PKCE loop; production always hits
            // `providers::login`.
            if test_login {
                store.set(
                    provider,
                    whycodes_auth::ProviderAuth {
                        method: "oauth".into(),
                        token: whycodes_auth::OAuthToken {
                            access_token: "test-access".into(),
                            refresh_token: None,
                            expires_at: None,
                            extra: Default::default(),
                        },
                    },
                )?;
            } else {
                whycodes_auth::providers::login(provider, &store, !no_browser).await?;
            }
            println!(
                "{}",
                logged_in_line(provider, &store.path().display().to_string())
            );
        }
        AuthCmd::Logout { provider } => {
            if store.remove(provider)? {
                println!("{}", logout_removed_line(provider));
            } else {
                println!("{}", logout_missing_line(provider));
            }
        }
        AuthCmd::Status => {
            let entries = store.list()?;
            if entries.is_empty() {
                println!("{}", auth_status_empty_line(&oauth_provider_list()));
            } else {
                println!("{} OAuth logins ({}):", "🔑".bold(), store.path().display());
                for (name, auth) in entries {
                    println!(
                        "  {:<15} {} · {}",
                        name.cyan(),
                        auth.method,
                        auth_expiry_label(&auth).dimmed()
                    );
                }
            }
        }
        AuthCmd::Import => cmd_auth_import(&data_dir).await?,
    }
    Ok(())
}

/// `auth import` — scan for other CLIs' credential files, ask once per new
/// source (the decision is persisted), import approved ones. Sources are
/// only ever read, never modified; symlinks are refused.
pub(crate) async fn cmd_auth_import(data_dir: &std::path::Path) -> anyhow::Result<()> {
    use whycodes_auth::discover::{ConsentStore, SourceState, import, scan};

    let consent = ConsentStore::new(data_dir);
    let found = scan(&consent);
    if found.is_empty() {
        println!(
            "{}",
            import_none_found_line(
                &whycodes_auth::discover::KNOWN_SOURCES
                    .iter()
                    .map(|s| s.label)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        );
        return Ok(());
    }

    println!(
        "{} Found credentials (a source is read only after your approval, and never modified):",
        "🔍".bold()
    );
    for f in &found {
        let state = match f.state {
            SourceState::New => "new".yellow(),
            SourceState::Approved => "approved".green(),
            SourceState::Denied => "denied".dimmed(),
            SourceState::Symlink => "symlink — refused".red(),
        };
        println!(
            "  {:<15} {:<45} {}",
            f.source.label.cyan(),
            f.path.display().to_string().dimmed(),
            state
        );
    }
    println!();

    let store = whycodes_auth::TokenStore::new(data_dir);
    let mut imported = 0usize;
    for f in &found {
        match f.state {
            SourceState::Symlink | SourceState::Denied => {}
            SourceState::Approved => match import(&store, &consent, f) {
                Ok(()) => {
                    imported += 1;
                    println!(
                        "{} Imported {} → `{}`",
                        "✓".green(),
                        f.source.label.cyan(),
                        f.source.provider
                    );
                }
                Err(e) => println!("{} {}: {e}", "✗".red(), f.source.label),
            },
            SourceState::New => {
                print!(
                    "Import {} ({}) as `{}`? [y/N] ",
                    f.source.label,
                    f.path.display(),
                    f.source.provider
                );
                use std::io::Write as _;
                if let Err(e) = std::io::stdout().flush() {
                    eprintln!("warning: could not flush prompt: {e}");
                }
                let mut line = String::new();
                super::helpers::read_repl_line(&mut line)?;
                let answer = line;
                let yes = import_prompt_yes(&answer);
                consent.record(&f.path, yes)?;
                if yes {
                    match import(&store, &consent, f) {
                        Ok(()) => {
                            imported += 1;
                            println!("{} Imported `{}`", "✓".green(), f.source.provider);
                        }
                        Err(e) => println!("{} {}: {e}", "✗".red(), f.source.label),
                    }
                } else {
                    println!(
                        "{}",
                        skipped_consent_line(&consent.path().display().to_string())
                    );
                }
            }
        }
    }
    if imported > 0 {
        println!("{}", imported_count_line(imported));
    }
    Ok(())
}

pub(crate) fn logged_in_line(provider: &str, store_path: &str) -> String {
    format!(
        "{} Logged in to {} — credential stored in {store_path}",
        "✓".green(),
        provider.cyan()
    )
}

pub(crate) fn logout_removed_line(provider: &str) -> String {
    format!(
        "{} Removed stored credentials for {}",
        "✓".green(),
        provider.cyan()
    )
}

pub(crate) fn logout_missing_line(provider: &str) -> String {
    format!("No stored credentials for `{provider}`.")
}

pub(crate) fn auth_status_empty_line(list: &str) -> String {
    format!("No OAuth logins yet. Run: whycodes auth login <{list}>")
}

pub(crate) fn import_none_found_line(looked_for: &str) -> String {
    format!("No credentials from other CLIs found (looked for {looked_for}).")
}

pub(crate) fn import_prompt_yes(answer: &str) -> bool {
    matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}

pub(crate) fn skipped_consent_line(consent_path: &str) -> String {
    format!("Skipped (won't ask again — delete {consent_path} to reset)")
}

pub(crate) fn imported_count_line(imported: usize) -> String {
    format!(
        "\n{} {imported} credential(s) ready — `whycodes auth status` lists them.",
        "✓".green()
    )
}

/// Human expiry label for `auth status` / `debug` — never token material.
pub(crate) fn auth_expiry_label(auth: &whycodes_auth::ProviderAuth) -> String {
    // A derived API token (e.g. Copilot's) is the one that actually expires;
    // it lives in extra. "copilot_expires_at" is the pre-rename key name.
    let derived_expiry = auth
        .token
        .extra
        .get("derived_expires_at")
        .or_else(|| auth.token.extra.get("copilot_expires_at"))
        .and_then(|v| v.as_str());
    if let Some(at) = derived_expiry {
        return format!("derived API token expires {at}");
    }
    match auth.token.expires_at {
        Some(at) => {
            if auth.token.is_expired() {
                format!("expired {at} (refreshes on next use)")
            } else {
                format!("expires {at}")
            }
        }
        None => "no expiry".to_string(),
    }
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
