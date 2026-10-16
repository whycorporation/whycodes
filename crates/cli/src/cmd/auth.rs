//! Auth login/logout/import.
use super::helpers::*;
use crate::args::*;
use colored::*;
use serde::Serialize;
use whycodes_config::Config;

/// `Config::data_dir` only wraps `paths::data_dir()` and does not fail today.
/// The `.` fallback stays so a future `Err` still has a directory to open.
pub(crate) fn auth_data_dir_from(
    dir: Result<std::path::PathBuf, whycodes_core::Error>,
) -> std::path::PathBuf {
    dir.unwrap_or_else(|_| std::path::PathBuf::from("."))
}

pub(crate) fn auth_data_dir() -> std::path::PathBuf {
    auth_data_dir_from(Config::data_dir())
}

pub(crate) async fn cmd_auth(project_dir: &std::path::Path, cmd: &AuthCmd) -> anyhow::Result<()> {
    let data_dir = auth_data_dir();
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
                        validated_at: None,
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
        AuthCmd::Status { json: true } => {
            let config = Config::load_layered(project_dir)?;
            let entries = auth_status_entries(
                &config,
                &store.list()?,
                |k| std::env::var(k).ok(),
                chrono::Utc::now(),
            );
            println!("{}", serde_json::to_string_pretty(&entries)?);
        }
        AuthCmd::Status { json: false } => {
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
        let state = import_state_label(f.state);
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
                    println!("{}", skipped_import(&consent));
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

fn import_state_label(state: whycodes_auth::discover::SourceState) -> colored::ColoredString {
    use whycodes_auth::discover::SourceState;
    match state {
        SourceState::New => "new".yellow(),
        SourceState::Approved => "approved".green(),
        SourceState::Denied => "denied".dimmed(),
        SourceState::Symlink => "symlink — refused".red(),
    }
}

fn skipped_import(consent: &whycodes_auth::discover::ConsentStore) -> String {
    skipped_consent_line(&consent.path().display().to_string())
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

/// One row of `auth status --json`: what the CLI would use for the
/// provider and, for a stored login, how fresh it is. Never token material.
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthStatusEntry {
    pub provider: String,
    /// `api_key` | `oauth` | `none`
    pub auth_kind: &'static str,
    /// `env:<VAR>` | `config` | `oauth_store` | `none` — the first hit in
    /// the CLI's own precedence (env var → config `api_key` → OAuth store).
    pub credential_source: String,
    /// OAuth only: when the access token (or derived API token) expires.
    pub expiry: Option<String>,
    /// OAuth only: when the login was last obtained or renewed.
    pub validated_at: Option<String>,
    /// OAuth only: not renewed for over a week (`whycodes_auth::token::STALE_AFTER`).
    pub stale: bool,
}

/// Every provider the config names or the store holds, in name order.
pub(crate) fn auth_status_entries(
    config: &Config,
    stored: &[(String, whycodes_auth::ProviderAuth)],
    env: impl Fn(&str) -> Option<String>,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<AuthStatusEntry> {
    let names: std::collections::BTreeSet<&str> = config
        .providers
        .keys()
        .map(String::as_str)
        .chain(stored.iter().map(|(name, _)| name.as_str()))
        .collect();
    names
        .into_iter()
        .map(|provider| {
            if let Some(source) = api_key_source(provider, config, &env) {
                return AuthStatusEntry {
                    provider: provider.to_string(),
                    auth_kind: "api_key",
                    credential_source: source,
                    expiry: None,
                    validated_at: None,
                    stale: false,
                };
            }
            match stored.iter().find(|(name, _)| name == provider) {
                Some((_, auth)) => AuthStatusEntry {
                    provider: provider.to_string(),
                    auth_kind: "oauth",
                    credential_source: "oauth_store".into(),
                    expiry: oauth_expiry(auth),
                    validated_at: auth.validated_at.map(|t| t.to_rfc3339()),
                    stale: auth.is_stale_at(now),
                },
                None => AuthStatusEntry {
                    provider: provider.to_string(),
                    auth_kind: "none",
                    credential_source: "none".into(),
                    expiry: None,
                    validated_at: None,
                    stale: false,
                },
            }
        })
        .collect()
}

/// The env var or config key an explicit API key would come from, in the
/// order `credential_candidates` tries them: the provider's default env
/// var, named credential lanes, config `api_key`, then `OPENAI_API_KEY`
/// for `openai`.
fn api_key_source(
    provider: &str,
    config: &Config,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let set = |var: &str| env(var).is_some_and(|v| !v.is_empty());
    let default_var = provider_env_var(provider);
    if set(&default_var) {
        return Some(format!("env:{default_var}"));
    }
    if let Some(pc) = config.get_provider(provider) {
        let ci = whycodes_core::types::process_is_ci_from(env);
        for name in pc.credentials.names_for_lane(ci) {
            let var = pc.credentials.env_var_for(&name, provider);
            if set(&var) {
                return Some(format!("env:{var}"));
            }
        }
        if pc.api_key.as_deref().is_some_and(|k| !k.is_empty()) {
            return Some("config".into());
        }
    }
    if provider == "openai" && set("OPENAI_API_KEY") {
        return Some("env:OPENAI_API_KEY".into());
    }
    None
}

/// RFC 3339 expiry of the credential the API actually receives: a derived
/// token's own expiry when the provider issues one, else the access token's.
fn oauth_expiry(auth: &whycodes_auth::ProviderAuth) -> Option<String> {
    auth.token
        .extra
        .get("derived_expires_at")
        .or_else(|| auth.token.extra.get("copilot_expires_at"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| auth.token.expires_at.map(|t| t.to_rfc3339()))
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
