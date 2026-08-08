//! Per-provider OAuth specifications, login flows, and transparent refresh.
//!
//! The flows reuse the public, pre-registered OAuth client ids that
//! first-party and community terminal agents already ship (Claude Code,
//! Codex CLI, Gemini CLI, VS Code's GitHub client). whycode cannot register
//! its own client for these providers, so subscription login rides on the
//! same identifiers a user's first-party CLI would use.
//!
//! Flow shape per provider:
//! - `anthropic` — PKCE, browser. The public Claude client's registered
//!   redirect is a console page that displays `code#state`; the user pastes
//!   it back into the terminal.
//! - `openai` — PKCE, browser → loopback callback on the fixed port the
//!   Codex client has registered (`localhost:1455/auth/callback`).
//! - `google` — PKCE, browser → loopback callback on an ephemeral port
//!   (Google permits any loopback port for installed-app clients).
//! - `github-copilot` — GitHub device-code flow (the only grant GitHub
//!   offers this client), then the GitHub token is exchanged for the
//!   short-lived Copilot API token.
//!
//! Security: tokens are printed nowhere. URLs contain only the PKCE
//! challenge (never the verifier). The verifier and tokens stay in memory
//! or in the 0600 store.

use std::io::Write as _;
use std::path::Path;
use std::time::Duration;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::OAUTH_PROVIDERS;
use crate::error::{AuthError, Result};
use crate::flow;
use crate::pkce::Pkce;
use crate::store::TokenStore;
use crate::token::{OAuthToken, ProviderAuth};

/// Bound on the whole browser step so a closed tab cannot hang login forever.
const BROWSER_FLOW_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Device-flow polls stop after the provider's own `expires_in` (15 min cap).
const DEVICE_FLOW_MAX: Duration = Duration::from_secs(15 * 60);

/// How a provider completes the browser step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowKind {
    /// Browser → provider redirects to a loopback listener we host.
    LoopbackPkce,
    /// Browser shows `code#state` on a provider page; the user pastes it
    /// back (used when the public client's registered redirect is fixed and
    /// is not a loopback address).
    PasteCodePkce,
    /// GitHub device-code grant (user enters a short code on github.com).
    DeviceCode,
}

/// Static description of one provider's OAuth endpoints.
pub struct ProviderSpec {
    pub name: &'static str,
    pub label: &'static str,
    pub flow: FlowKind,
    pub client_id: &'static str,
    /// Installed-app client secret where the provider requires one (Google).
    /// These are public by design — they ship in plaintext in the
    /// first-party open-source CLIs.
    pub client_secret: Option<&'static str>,
    pub authorize_url: &'static str,
    pub token_url: &'static str,
    pub scopes: &'static str,
    /// Fixed loopback port when the registered redirect demands one
    /// (OpenAI). `None` → bind an ephemeral port.
    pub loopback_port: Option<u16>,
    /// Path the loopback listener answers on.
    pub callback_path: &'static str,
    /// Extra authorize-url query pairs (provider-specific switches).
    pub extra_authorize: &'static [(&'static str, &'static str)],
}

/// Look up the OAuth spec for a provider name.
pub fn spec_for(provider: &str) -> Result<ProviderSpec> {
    match provider {
        // Public Claude Code client. The registered redirect is the console
        // page that displays the code — hence the paste flow.
        "anthropic" => Ok(ProviderSpec {
            name: "anthropic",
            label: "Anthropic (Claude Pro/Max)",
            flow: FlowKind::PasteCodePkce,
            client_id: "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
            client_secret: None,
            authorize_url: "https://claude.ai/oauth/authorize",
            token_url: "https://console.anthropic.com/v1/oauth/token",
            scopes: "org:create_api_key user:profile user:inference",
            loopback_port: None,
            callback_path: "",
            extra_authorize: &[("code", "true")],
        }),
        // Public Codex CLI client. Redirect is registered as
        // http://localhost:1455/auth/callback — the port is not optional.
        "openai" => Ok(ProviderSpec {
            name: "openai",
            label: "OpenAI (ChatGPT Plus/Pro)",
            flow: FlowKind::LoopbackPkce,
            client_id: "app_EMoamEEZ73f0CkXaXp7hrann",
            client_secret: None,
            authorize_url: "https://auth.openai.com/oauth/authorize",
            token_url: "https://auth.openai.com/oauth/token",
            scopes: "openid profile email offline_access",
            loopback_port: Some(1455),
            callback_path: "/auth/callback",
            extra_authorize: &[
                ("id_token_add_organizations", "true"),
                ("codex_cli_simplified_flow", "true"),
            ],
        }),
        // Public Gemini CLI installed-app client. Any loopback port works.
        "google" => Ok(ProviderSpec {
            name: "google",
            label: "Google (Gemini)",
            flow: FlowKind::LoopbackPkce,
            client_id: "REDACTED_GEMINI_CLI_CLIENT_ID",
            client_secret: Some("REDACTED_GEMINI_CLI_CLIENT_SECRET"),
            authorize_url: "https://accounts.google.com/o/oauth2/v2/auth",
            token_url: "https://oauth2.googleapis.com/token",
            scopes: "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile",
            loopback_port: None,
            callback_path: "/oauth2callback",
            extra_authorize: &[("access_type", "offline"), ("prompt", "consent")],
        }),
        // Public VS Code GitHub client (device flow enabled). Copilot API
        // access comes from exchanging the GitHub token afterwards.
        "github-copilot" => Ok(ProviderSpec {
            name: "github-copilot",
            label: "GitHub Copilot",
            flow: FlowKind::DeviceCode,
            client_id: "Iv1.b507a08c87ecfe98",
            client_secret: None,
            authorize_url: "https://github.com/login/device/code",
            token_url: "https://github.com/login/oauth/access_token",
            scopes: "read:user",
            loopback_port: None,
            callback_path: "",
            extra_authorize: &[],
        }),
        other => Err(AuthError::UnsupportedProvider(other.to_string())),
    }
}

/// Run the full login flow for `provider` and persist the credential.
///
/// Prints user-facing instructions (URLs, device codes) on stdout; never
/// prints token material. When `open_browser` is false the URL is only
/// printed for manual use.
pub async fn login(provider: &str, store: &TokenStore, open_browser: bool) -> Result<ProviderAuth> {
    let spec = spec_for(provider)?;
    let token = match spec.flow {
        FlowKind::LoopbackPkce => loopback_login(&spec, open_browser).await?,
        FlowKind::PasteCodePkce => paste_code_login(&spec, open_browser).await?,
        FlowKind::DeviceCode => device_login(&spec, open_browser).await?,
    };
    let auth = ProviderAuth {
        method: "oauth".to_string(),
        token,
    };
    store.set(spec.name, auth.clone())?;
    Ok(auth)
}

/// Return a usable API credential for `provider` from the store under
/// `data_dir`, refreshing it first when expired. `None` when not logged in
/// or refresh fails (the caller then falls back to its normal error path).
///
/// For `github-copilot` this is the short-lived Copilot API token, not the
/// underlying GitHub OAuth token.
pub async fn access_token(provider: &str, data_dir: &Path) -> Option<String> {
    let spec = spec_for(provider).ok()?;
    let store = TokenStore::new(data_dir);
    let auth = store.get(spec.name).ok()??;
    let token = ensure_fresh(&spec, &store, auth.token).await.ok()?;
    usable_token(&spec, &token)
}

/// Load + refresh if needed and return the token; kept separate so a
/// refresh failure never deletes a stored credential.
async fn ensure_fresh(
    spec: &ProviderSpec,
    store: &TokenStore,
    token: OAuthToken,
) -> Result<OAuthToken> {
    if spec.name == "github-copilot" {
        return ensure_fresh_copilot(spec, store, token).await;
    }
    if !token.is_expired() {
        return Ok(token);
    }
    let Some(refresh) = token.refresh_token.clone() else {
        // Expired and no way to renew — the user must log in again.
        return Err(AuthError::NotLoggedIn(spec.name.to_string()));
    };
    tracing::debug!(
        provider = spec.name,
        "OAuth access token expired; refreshing"
    );
    let refreshed = refresh_grant(spec, &refresh)
        .await
        .map_err(|e| AuthError::Refresh(spec.name.to_string(), e.to_string()))?;
    store.set(
        spec.name,
        ProviderAuth {
            method: "oauth".to_string(),
            token: refreshed.clone(),
        },
    )?;
    Ok(refreshed)
}

/// The credential sent to the provider's API.
fn usable_token(spec: &ProviderSpec, token: &OAuthToken) -> Option<String> {
    if spec.name == "github-copilot" {
        return token
            .extra
            .get("copilot_token")
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    Some(token.access_token.clone())
}

// ────────────────────────────────────────────────────────────────────────
// Browser + localhost callback (OpenAI, Google)
// ────────────────────────────────────────────────────────────────────────

async fn loopback_login(spec: &ProviderSpec, open_browser: bool) -> Result<OAuthToken> {
    let pkce = Pkce::new();
    let listener = match spec.loopback_port {
        Some(port) => std::net::TcpListener::bind(("127.0.0.1", port)).map_err(|e| {
            AuthError::FlowCancelled(format!(
                "port {port} is busy; the {} redirect is registered for that port — free it and retry ({e})",
                spec.name
            ))
        })?,
        None => flow::bind_loopback()?.0,
    };
    let port = listener.local_addr().map_err(AuthError::Io)?.port();
    let redirect_uri = format!("http://localhost:{port}{}", spec.callback_path);
    let url = flow::authorize_url(
        spec.authorize_url,
        spec.client_id,
        &redirect_uri,
        spec.scopes,
        &pkce,
        spec.extra_authorize,
    );

    println!("Open this URL to log in with {}:\n\n  {url}\n", spec.label);
    if open_browser {
        match flow::open_browser(&url) {
            Ok(()) => println!("Browser opened — complete the sign-in there."),
            Err(AuthError::BrowserUnavailable(_)) => {
                println!("(Could not open a browser; visit the URL manually.)")
            }
            Err(e) => return Err(e),
        }
    }
    println!("Waiting for the sign-in to complete…");
    std::io::stdout().flush().ok();

    let expected_state = pkce.state.clone();
    let callback = tokio::task::spawn_blocking(move || {
        flow::wait_for_callback(&listener, &expected_state, BROWSER_FLOW_TIMEOUT)
    })
    .await
    .map_err(|e| AuthError::FlowCancelled(format!("callback task failed: {e}")))??;

    exchange_code(spec, &callback.code, &redirect_uri, &pkce).await
}

// ────────────────────────────────────────────────────────────────────────
// Browser + paste `code#state` (Anthropic)
// ────────────────────────────────────────────────────────────────────────

async fn paste_code_login(spec: &ProviderSpec, open_browser: bool) -> Result<OAuthToken> {
    let pkce = Pkce::new();
    // The public Claude client's registered redirect — a console page that
    // displays the code. It is not a loopback address, hence the paste step.
    let redirect_uri = "https://console.anthropic.com/oauth/code/callback";
    let url = flow::authorize_url(
        spec.authorize_url,
        spec.client_id,
        redirect_uri,
        spec.scopes,
        &pkce,
        spec.extra_authorize,
    );

    println!("Open this URL to log in with {}:\n\n  {url}\n", spec.label);
    if open_browser {
        match flow::open_browser(&url) {
            Ok(()) => println!("Browser opened — complete the sign-in there."),
            Err(AuthError::BrowserUnavailable(_)) => {
                println!("(Could not open a browser; visit the URL manually.)")
            }
            Err(e) => return Err(e),
        }
    }
    println!("After signing in, the browser shows a code. Paste it here:");
    print!("> ");
    std::io::stdout().flush().ok();

    let pasted = tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).map(|_| line)
    })
    .await
    .map_err(|e| AuthError::FlowCancelled(format!("stdin task failed: {e}")))?
    .map_err(AuthError::Io)?;
    let pasted = pasted.trim();
    if pasted.is_empty() {
        return Err(AuthError::FlowCancelled("no code pasted".to_string()));
    }

    // The console page shows `code#state`.
    let (code, state) = match pasted.split_once('#') {
        Some((c, s)) => (c.to_string(), s.to_string()),
        None => (pasted.to_string(), String::new()),
    };
    if !state.is_empty() && state != pkce.state {
        return Err(AuthError::Provider(
            "state mismatch on pasted code (possible CSRF); aborting".to_string(),
        ));
    }
    exchange_code(spec, &code, redirect_uri, &pkce).await
}

// ────────────────────────────────────────────────────────────────────────
// GitHub device flow + Copilot token exchange
// ────────────────────────────────────────────────────────────────────────

async fn device_login(spec: &ProviderSpec, open_browser: bool) -> Result<OAuthToken> {
    let client = http_client()?;
    let resp = client
        .post(spec.authorize_url)
        .header("Accept", "application/json")
        .form(&[("client_id", spec.client_id), ("scope", spec.scopes)])
        .send()
        .await?
        .error_for_status()
        .map_err(AuthError::Http)?
        .json::<Value>()
        .await?;

    let device_code = resp["device_code"]
        .as_str()
        .ok_or_else(|| AuthError::Provider("device flow: missing device_code".to_string()))?
        .to_string();
    let user_code = resp["user_code"].as_str().unwrap_or("").to_string();
    let verification_uri = resp["verification_uri"]
        .as_str()
        .unwrap_or("https://github.com/login/device")
        .to_string();
    let mut interval = resp["interval"].as_u64().unwrap_or(5).max(1);
    let expires_in = resp["expires_in"].as_u64().unwrap_or(900);

    println!("\nGitHub Copilot login:");
    println!("  1. Visit  {verification_uri}");
    println!("  2. Enter code:  {user_code}\n");
    if open_browser {
        match flow::open_browser(&verification_uri) {
            Ok(()) => println!("Browser opened — enter the code there."),
            Err(AuthError::BrowserUnavailable(_)) => {
                println!("(Could not open a browser; visit the URL manually.)")
            }
            Err(e) => return Err(e),
        }
    }
    println!("Waiting for authorization…");
    std::io::stdout().flush().ok();

    let deadline = std::time::Instant::now() + DEVICE_FLOW_MAX.min(Duration::from_secs(expires_in));
    let github_token = loop {
        if std::time::Instant::now() > deadline {
            return Err(AuthError::FlowCancelled(
                "device code expired before authorization".to_string(),
            ));
        }
        tokio::time::sleep(Duration::from_secs(interval)).await;
        let poll = client
            .post(spec.token_url)
            .header("Accept", "application/json")
            .form(&[
                ("client_id", spec.client_id),
                ("device_code", device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await?
            .json::<Value>()
            .await?;
        match poll["error"].as_str() {
            None => {
                let token = poll["access_token"]
                    .as_str()
                    .ok_or_else(|| {
                        AuthError::TokenExchange("device flow: missing access_token".to_string())
                    })?
                    .to_string();
                break token;
            }
            // Keep waiting; `slow_down` asks us to add 5s (RFC 8628 §3.5).
            Some("authorization_pending") => {}
            Some("slow_down") => interval += 5,
            Some("expired_token") => {
                return Err(AuthError::FlowCancelled(
                    "device code expired; run login again".to_string(),
                ));
            }
            Some(other) => {
                let desc = poll["error_description"].as_str().unwrap_or(other);
                return Err(AuthError::Provider(desc.to_string()));
            }
        }
    };

    // Exchange the GitHub token for the short-lived Copilot API token.
    let mut token = OAuthToken {
        access_token: github_token,
        refresh_token: None,
        expires_at: None, // GitHub OAuth tokens from the device flow do not expire
        extra: Default::default(),
    };
    let (copilot_token, copilot_expires) = exchange_copilot_token(&token.access_token).await?;
    set_copilot_extra(&mut token, &copilot_token, copilot_expires);
    Ok(token)
}

/// GET the Copilot API token for a GitHub OAuth token.
async fn exchange_copilot_token(github_token: &str) -> Result<(String, DateTime<Utc>)> {
    let resp = http_client()?
        .get("https://api.github.com/copilot_internal/v2/token")
        .header("Accept", "application/json")
        .header("Authorization", format!("token {github_token}"))
        .header("Editor-Version", "vscode/1.95.0")
        .send()
        .await?;
    let status = resp.status();
    let json: Value = resp.json().await?;
    if !status.is_success() {
        let msg = json["message"].as_str().unwrap_or("unknown error");
        return Err(AuthError::TokenExchange(format!(
            "Copilot token exchange failed ({status}): {msg} — is Copilot enabled for this GitHub account?"
        )));
    }
    let token = json["token"]
        .as_str()
        .ok_or_else(|| AuthError::TokenExchange("Copilot exchange: missing token".to_string()))?
        .to_string();
    let expires_at = json["expires_at"]
        .as_i64()
        .and_then(|secs| DateTime::from_timestamp(secs, 0))
        .ok_or_else(|| {
            AuthError::TokenExchange("Copilot exchange: missing expires_at".to_string())
        })?;
    Ok((token, expires_at))
}

fn set_copilot_extra(token: &mut OAuthToken, copilot_token: &str, expires: DateTime<Utc>) {
    token.extra.insert(
        "copilot_token".to_string(),
        Value::String(copilot_token.to_string()),
    );
    token.extra.insert(
        "copilot_expires_at".to_string(),
        Value::String(expires.to_rfc3339()),
    );
}

/// The Copilot API token lives in `extra`; re-exchange when it is near
/// expiry. The underlying GitHub token itself does not expire.
async fn ensure_fresh_copilot(
    spec: &ProviderSpec,
    store: &TokenStore,
    mut token: OAuthToken,
) -> Result<OAuthToken> {
    let fresh = token
        .extra
        .get("copilot_expires_at")
        .and_then(Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|at| Utc::now() + chrono::Duration::seconds(60) < at.with_timezone(&Utc))
        .unwrap_or(false);
    if fresh && token.extra.contains_key("copilot_token") {
        return Ok(token);
    }
    tracing::debug!("Copilot API token expired; re-exchanging");
    let (copilot_token, expires) = exchange_copilot_token(&token.access_token).await?;
    set_copilot_extra(&mut token, &copilot_token, expires);
    store.set(
        spec.name,
        ProviderAuth {
            method: "oauth".to_string(),
            token: token.clone(),
        },
    )?;
    Ok(token)
}

// ────────────────────────────────────────────────────────────────────────
// Token endpoint helpers
// ────────────────────────────────────────────────────────────────────────

/// Authorization-code exchange (PKCE). Anthropic speaks JSON at its token
/// endpoint; OpenAI and Google use the standard form encoding.
async fn exchange_code(
    spec: &ProviderSpec,
    code: &str,
    redirect_uri: &str,
    pkce: &Pkce,
) -> Result<OAuthToken> {
    let client = http_client()?;
    let resp = if spec.name == "anthropic" {
        client
            .post(spec.token_url)
            .json(&serde_json::json!({
                "grant_type": "authorization_code",
                "client_id": spec.client_id,
                "code": code,
                "state": pkce.state,
                "redirect_uri": redirect_uri,
                "code_verifier": pkce.verifier,
            }))
            .send()
            .await?
    } else {
        let mut form: Vec<(&str, &str)> = vec![
            ("grant_type", "authorization_code"),
            ("client_id", spec.client_id),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", pkce.verifier.as_str()),
        ];
        if let Some(secret) = spec.client_secret {
            form.push(("client_secret", secret));
        }
        client
            .post(spec.token_url)
            .header("Accept", "application/json")
            .form(&form)
            .send()
            .await?
    };
    parse_token_response(resp).await
}

/// Refresh-token grant for providers that issue refresh tokens.
async fn refresh_grant(spec: &ProviderSpec, refresh_token: &str) -> Result<OAuthToken> {
    let client = http_client()?;
    let resp = if spec.name == "anthropic" {
        client
            .post(spec.token_url)
            .json(&serde_json::json!({
                "grant_type": "refresh_token",
                "client_id": spec.client_id,
                "refresh_token": refresh_token,
            }))
            .send()
            .await?
    } else {
        let mut form: Vec<(&str, &str)> = vec![
            ("grant_type", "refresh_token"),
            ("client_id", spec.client_id),
            ("refresh_token", refresh_token),
        ];
        if let Some(secret) = spec.client_secret {
            form.push(("client_secret", secret));
        }
        client
            .post(spec.token_url)
            .header("Accept", "application/json")
            .form(&form)
            .send()
            .await?
    };
    parse_token_response(resp).await
}

/// Parse a token-endpoint response, surfacing provider errors without
/// leaking response bodies that could contain token material.
async fn parse_token_response(resp: reqwest::Response) -> Result<OAuthToken> {
    let status = resp.status();
    let json: Value = resp.json().await?;
    if !status.is_success() {
        let msg = json["error_description"]
            .as_str()
            .or_else(|| json["error"]["message"].as_str())
            .or_else(|| json["error"].as_str())
            .unwrap_or("unknown error");
        return Err(AuthError::TokenExchange(format!("({status}) {msg}")));
    }
    token_from_json(&json)
}

/// Shared response → `OAuthToken` mapping. `expires_in` may arrive as a
/// number or a string depending on the provider.
fn token_from_json(json: &Value) -> Result<OAuthToken> {
    let access_token = json["access_token"]
        .as_str()
        .ok_or_else(|| AuthError::TokenExchange("response missing access_token".to_string()))?
        .to_string();
    let refresh_token = json["refresh_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let expires_in = json["expires_in"]
        .as_i64()
        .or_else(|| json["expires_in"].as_str().and_then(|s| s.parse().ok()));
    let expires_at = expires_in.map(|secs| Utc::now() + chrono::Duration::seconds(secs));

    let mut extra = serde_json::Map::new();
    if let Some(account) = json["id_token"]
        .as_str()
        .and_then(openai_account_id_from_jwt)
    {
        extra.insert("openai_account_id".to_string(), Value::String(account));
    }

    Ok(OAuthToken {
        access_token,
        refresh_token,
        expires_at,
        extra,
    })
}

/// Extract `chatgpt_account_id` from a Codex `id_token` JWT (payload only;
/// the channel was already authenticated by the TLS token exchange). Stored
/// for the Codex backend route, which needs it as a header.
fn openai_account_id_from_jwt(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let json: Value = serde_json::from_slice(&bytes).ok()?;
    json["https://api.openai.com/auth"]["chatgpt_account_id"]
        .as_str()
        .map(str::to_string)
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(concat!("whycode/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(AuthError::Http)
}

/// True when `provider` has an OAuth flow at all (for CLI validation).
pub fn supports_oauth(provider: &str) -> bool {
    OAUTH_PROVIDERS.contains(&provider)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_exist_for_all_advertised_providers() {
        for name in OAUTH_PROVIDERS {
            let spec = spec_for(name).expect(name);
            assert_eq!(spec.name, *name);
            assert!(!spec.client_id.is_empty());
            assert!(spec.authorize_url.starts_with("https://"));
            assert!(spec.token_url.starts_with("https://"));
        }
    }

    #[test]
    fn unknown_provider_is_rejected() {
        assert!(matches!(
            spec_for("mistral"),
            Err(AuthError::UnsupportedProvider(_))
        ));
        assert!(!supports_oauth("mistral"));
        assert!(supports_oauth("anthropic"));
    }

    #[test]
    fn flow_kinds_match_registered_redirects() {
        // These encode deployment facts about the public clients; if a
        // provider changes its registration this test is the tripwire.
        assert_eq!(spec_for("openai").unwrap().flow, FlowKind::LoopbackPkce);
        assert_eq!(spec_for("openai").unwrap().loopback_port, Some(1455));
        assert_eq!(spec_for("google").unwrap().flow, FlowKind::LoopbackPkce);
        assert_eq!(spec_for("anthropic").unwrap().flow, FlowKind::PasteCodePkce);
        assert_eq!(
            spec_for("github-copilot").unwrap().flow,
            FlowKind::DeviceCode
        );
    }

    #[test]
    fn token_from_json_accepts_string_or_number_expires_in() {
        let t = token_from_json(&serde_json::json!({
            "access_token": "a", "refresh_token": "r", "expires_in": 3600
        }))
        .unwrap();
        assert!(t.expires_at.is_some());
        assert_eq!(t.refresh_token.as_deref(), Some("r"));
        let t = token_from_json(&serde_json::json!({
            "access_token": "a", "expires_in": "3600"
        }))
        .unwrap();
        assert!(t.expires_at.is_some());
        assert!(t.refresh_token.is_none());
    }

    #[test]
    fn token_from_json_requires_access_token() {
        let err = token_from_json(&serde_json::json!({"token_type": "Bearer"})).unwrap_err();
        assert!(matches!(err, AuthError::TokenExchange(_)));
    }

    #[test]
    fn openai_account_id_is_extracted_from_id_token() {
        // Synthetic JWT payload with the Codex account claim.
        let payload = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&serde_json::json!({
                "https://api.openai.com/auth": {"chatgpt_account_id": "acct_1"}
            }))
            .unwrap(),
        );
        let jwt = format!("header.{payload}.sig");
        assert_eq!(openai_account_id_from_jwt(&jwt).as_deref(), Some("acct_1"));
        assert!(openai_account_id_from_jwt("not-a-jwt").is_none());

        // token_from_json picks it up into `extra`.
        let t = token_from_json(&serde_json::json!({
            "access_token": "a", "id_token": jwt
        }))
        .unwrap();
        assert_eq!(
            t.extra.get("openai_account_id").and_then(Value::as_str),
            Some("acct_1")
        );
    }

    #[test]
    fn copilot_usable_token_comes_from_extra() {
        let spec = spec_for("github-copilot").unwrap();
        let mut token = OAuthToken {
            access_token: "gh".to_string(),
            refresh_token: None,
            expires_at: None,
            extra: Default::default(),
        };
        assert!(usable_token(&spec, &token).is_none());
        set_copilot_extra(&mut token, "cop", Utc::now() + chrono::Duration::hours(1));
        assert_eq!(usable_token(&spec, &token).as_deref(), Some("cop"));
    }
}
