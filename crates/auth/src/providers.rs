//! Generic OAuth login flows and transparent refresh.
//!
//! Provider client ids, redirect ports, and identity headers come from
//! auth plugins (`crate::spec`). Built-in WhyCodes ships an empty registry.

use std::io::{BufRead, Write as _};
use std::path::Path;
use std::time::Duration;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::error::{AuthError, Result};
use crate::flow;
use crate::pkce::Pkce;
use crate::spec::{DerivedCredential, FlowKind, ProviderSpec, TokenEncoding};
use crate::store::TokenStore;
use crate::token::{OAuthToken, ProviderAuth};

/// Bound on the whole browser step so a closed tab cannot hang login forever.
/// Tests use a short window so unused `CliLoginUi` instantiations can finish.
const BROWSER_FLOW_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_millis(400)
} else {
    Duration::from_secs(5 * 60)
};
/// Device-flow polls stop after the provider's own `expires_in` (15 min cap).
const DEVICE_FLOW_MAX: Duration = Duration::from_secs(15 * 60);

/// User-interaction hooks for the login flows. The CLI implements this
/// with stdout/stdin ([`CliLoginUi`]); the TUI drives it from status lines
/// and the prompt box. Token material never passes through this interface.
pub trait LoginUi {
    /// Show the authorize URL; `browser_opened` reports whether the
    /// browser launch succeeded (the flow attempts it when requested).
    fn show_sign_in(&mut self, label: &str, url: &str, browser_opened: bool);
    /// Progress note ("waiting for the browser redirect", …).
    fn note(&mut self, text: &str);
    /// Device flow: the code the user must enter at `verification_uri`.
    fn show_device_code(&mut self, user_code: &str, verification_uri: &str, browser_opened: bool);
    /// Paste-code flow only: obtain the pasted `code#state`.
    fn prompt_pasted_code(&mut self) -> impl Future<Output = Result<String>> + Send;
}

/// stdout/stdin [`LoginUi`] used by `whycodes auth login`.
pub struct CliLoginUi;

impl LoginUi for CliLoginUi {
    fn show_sign_in(&mut self, label: &str, url: &str, browser_opened: bool) {
        println!("Open this URL to log in with {label}:\n\n  {url}\n");
        if browser_opened {
            println!("Browser opened — complete the sign-in there.");
        }
    }

    fn note(&mut self, text: &str) {
        println!("{text}");
        std::io::stdout().flush().ok();
    }

    fn show_device_code(&mut self, user_code: &str, verification_uri: &str, browser_opened: bool) {
        println!("\nGitHub Copilot login:");
        println!("  1. Visit  {verification_uri}");
        println!("  2. Enter code:  {user_code}\n");
        if browser_opened {
            println!("Browser opened — enter the code there.");
        }
    }

    async fn prompt_pasted_code(&mut self) -> Result<String> {
        println!("After signing in, the browser shows a code. Paste it here:");
        print!("> ");
        std::io::stdout().flush().ok();
        join_blocking_paste(
            tokio::task::spawn_blocking(|| read_pasted_code(&mut std::io::stdin().lock())).await,
        )
    }
}

/// Trim a pasted `code#state` line. Separated from stdin so tests do not
/// block on a TTY.
fn read_pasted_code(input: &mut dyn BufRead) -> Result<String> {
    let mut line = String::new();
    input.read_line(&mut line).map_err(AuthError::Io)?;
    Ok(line.trim().to_string())
}

fn join_blocking_paste(
    joined: std::result::Result<Result<String>, tokio::task::JoinError>,
) -> Result<String> {
    joined.unwrap_or_else(|error| {
        Err(AuthError::FlowCancelled(format!(
            "stdin task failed: {error}"
        )))
    })
}

fn join_blocking_callback(
    joined: std::result::Result<Result<flow::CallbackResult>, tokio::task::JoinError>,
) -> Result<flow::CallbackResult> {
    joined.unwrap_or_else(|error| {
        Err(AuthError::FlowCancelled(format!(
            "callback task failed: {error}"
        )))
    })
}

/// Run the full login flow for `provider` and persist the credential.
///
/// Prints user-facing instructions (URLs, device codes) on stdout; never
/// prints token material. When `open_browser` is false the URL is only
/// printed for manual use.
pub async fn login(provider: &str, store: &TokenStore, open_browser: bool) -> Result<ProviderAuth> {
    login_with_ui(provider, store, open_browser, &mut CliLoginUi).await
}

/// [`login`] with a caller-provided UI driver (TUI dialogs, tests).
pub async fn login_with_ui<U: LoginUi>(
    provider: &str,
    store: &TokenStore,
    open_browser: bool,
    ui: &mut U,
) -> Result<ProviderAuth> {
    let spec = spec_for(provider)?;
    login_with_spec(&spec, store, open_browser, ui).await
}

async fn login_with_spec<U: LoginUi>(
    spec: &ProviderSpec,
    store: &TokenStore,
    open_browser: bool,
    ui: &mut U,
) -> Result<ProviderAuth> {
    let token = if spec.name == "google-antigravity" {
        let oauth_token = loopback_login(spec, open_browser, ui).await?;
        crate::cca::perform_antigravity_onboarding(oauth_token).await?
    } else {
        match spec.flow {
            FlowKind::LoopbackPkce => loopback_login(spec, open_browser, ui).await?,
            FlowKind::PasteCodePkce => paste_code_login(spec, open_browser, ui).await?,
            FlowKind::DeviceCode => device_login(spec, open_browser, ui).await?,
        }
    };
    persist_token(store, spec.name.as_str(), "oauth", &token)?;
    Ok(ProviderAuth {
        method: "oauth".to_string(),
        token,
    })
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
    let auth = store.get(&spec.name).ok()??;
    let token = ensure_fresh(&spec, &store, &auth.method, auth.token)
        .await
        .ok()?;
    usable_token(&spec, &token)
}

/// Force a credential renewal regardless of the expiry window and return
/// the usable API credential.
///
/// Used after a provider answers 401 on a token the store considered fresh
/// (revoked server-side, clock skew, a derived token invalidated early).
/// How many times a rejection may trigger this is the *caller's* policy
/// (whycodes-llm does it once per request); this function only guarantees
/// the store ends up holding the newest credential the provider will issue.
/// `None` when not logged in, when the credential has no renewal path, or
/// when the provider refuses the renewal — the stored credential is never
/// deleted here, and the import method is preserved.
pub async fn force_refresh(provider: &str, data_dir: &Path) -> Option<String> {
    let spec = spec_for(provider).ok()?;
    force_refresh_with_spec(&spec, data_dir).await
}

async fn force_refresh_with_spec(spec: &ProviderSpec, data_dir: &Path) -> Option<String> {
    let store = TokenStore::new(data_dir);
    let auth = store.get(&spec.name).ok()??;
    let method = auth.method.clone();
    let token = force_fresh(spec, &store, &method, auth.token).await.ok()?;
    usable_token(spec, &token)
}

/// Like `ensure_fresh` but ignores the freshness window: always renew.
async fn force_fresh(
    spec: &ProviderSpec,
    store: &TokenStore,
    method: &str,
    token: OAuthToken,
) -> Result<OAuthToken> {
    if spec.derived.is_some() {
        tracing::debug!(
            provider = %spec.name,
            "derived API token rejected; forcing re-exchange"
        );
        return reexchange_derived(spec, store, method, token).await;
    }
    let Some(refresh) = token.refresh_token.clone() else {
        // No way to renew — the user must log in again.
        return Err(AuthError::NotLoggedIn(spec.name.to_string()));
    };
    tracing::debug!(
        provider = %spec.name,
        "OAuth credential rejected; forcing refresh"
    );
    let refreshed = refresh_grant(spec, &refresh)
        .await
        .map_err(|e| AuthError::Refresh(spec.name.to_string(), e.to_string()))?;
    persist_token(store, spec.name.as_str(), method, &refreshed)?;
    Ok(refreshed)
}

/// Load + refresh if needed and return the token; kept separate so a
/// refresh failure never deletes a stored credential.
async fn ensure_fresh(
    spec: &ProviderSpec,
    store: &TokenStore,
    method: &str,
    token: OAuthToken,
) -> Result<OAuthToken> {
    if spec.derived.is_some() {
        return ensure_fresh_derived(spec, store, method, token).await;
    }
    if !token.is_expired() {
        return Ok(token);
    }
    let Some(refresh) = token.refresh_token.clone() else {
        // Expired and no way to renew — the user must log in again.
        return Err(AuthError::NotLoggedIn(spec.name.to_string()));
    };
    tracing::debug!(
        provider = %spec.name,
        "OAuth access token expired; refreshing"
    );
    let refreshed = refresh_grant(spec, &refresh)
        .await
        .map_err(|e| AuthError::Refresh(spec.name.to_string(), e.to_string()))?;
    persist_token(store, spec.name.as_str(), method, &refreshed)?;
    Ok(refreshed)
}

/// The credential sent to the provider's API: the derived token when the
/// spec declares one, otherwise the access token itself.
fn usable_token(spec: &ProviderSpec, token: &OAuthToken) -> Option<String> {
    if spec.derived.is_some() {
        // "copilot_token" is the pre-rename key; read it so stores written
        return token
            .extra
            .get("derived_token")
            .or_else(|| token.extra.get("copilot_token"))
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    Some(token.access_token.clone())
}

// ────────────────────────────────────────────────────────────────────────
// Browser + localhost callback (OpenAI, Google)
// ────────────────────────────────────────────────────────────────────────

/// Open `url` in the browser; `Ok(false)` = no browser available (the UI
/// already has the URL for manual use).
fn try_open_browser(url: &str) -> bool {
    flow::open_browser(url).is_ok()
}

fn maybe_open_browser(open: bool, url: &str) -> bool {
    open && try_open_browser(url)
}

async fn loopback_login(
    spec: &ProviderSpec,
    open_browser: bool,
    ui: &mut impl LoginUi,
) -> Result<OAuthToken> {
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
    let host = spec.loopback_host.as_deref().unwrap_or("localhost");
    let redirect_uri = format!("http://{host}:{port}{}", spec.callback_path);
    let url = flow::authorize_url(
        &spec.authorize_url,
        &spec.client_id,
        &redirect_uri,
        &spec.scopes,
        &pkce,
        &spec.extra_authorize,
    );

    let opened = maybe_open_browser(open_browser, &url);
    ui.show_sign_in(&spec.label, &url, opened);
    ui.note("Waiting for the sign-in to complete…");

    let expected_state = pkce.state.clone();
    let joined = tokio::task::spawn_blocking(move || {
        flow::wait_for_callback(&listener, &expected_state, BROWSER_FLOW_TIMEOUT)
    })
    .await;
    let callback = join_blocking_callback(joined)?;

    exchange_code(spec, &callback.code, &redirect_uri, &pkce).await
}

// ────────────────────────────────────────────────────────────────────────
// Browser + paste `code#state` (Anthropic)
// ────────────────────────────────────────────────────────────────────────

async fn paste_code_login(
    spec: &ProviderSpec,
    open_browser: bool,
    ui: &mut impl LoginUi,
) -> Result<OAuthToken> {
    let pkce = Pkce::new();
    // Paste flows exist because the registered redirect is a fixed provider
    // page (not loopback) — `validate()` guarantees it is set.
    let redirect_uri = spec.redirect_uri.as_deref().ok_or_else(|| {
        AuthError::Provider(format!(
            "{}: paste-code flow needs a registered redirect_uri in the spec",
            spec.name
        ))
    })?;
    let url = flow::authorize_url(
        &spec.authorize_url,
        &spec.client_id,
        redirect_uri,
        &spec.scopes,
        &pkce,
        &spec.extra_authorize,
    );

    let opened = maybe_open_browser(open_browser, &url);
    ui.show_sign_in(&spec.label, &url, opened);
    let pasted = ui.prompt_pasted_code().await?;
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

async fn device_login(
    spec: &ProviderSpec,
    open_browser: bool,
    ui: &mut impl LoginUi,
) -> Result<OAuthToken> {
    let client = http_client()?;
    let resp = client
        .post(&spec.authorize_url)
        .header("Accept", "application/json")
        .form(&[
            ("client_id", spec.client_id.as_str()),
            ("scope", spec.scopes.as_str()),
        ])
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

    let opened = maybe_open_browser(open_browser, &verification_uri);
    ui.show_device_code(&user_code, &verification_uri, opened);
    ui.note("Waiting for authorization…");

    let deadline = std::time::Instant::now() + DEVICE_FLOW_MAX.min(Duration::from_secs(expires_in));
    let github_token = loop {
        if std::time::Instant::now() > deadline {
            return Err(AuthError::FlowCancelled(
                "device code expired before authorization".to_string(),
            ));
        }
        tokio::time::sleep(Duration::from_secs(interval)).await;
        let poll = client
            .post(&spec.token_url)
            .header("Accept", "application/json")
            .form(&[
                ("client_id", spec.client_id.as_str()),
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

    // Exchange the OAuth token for the derived API credential (e.g. the
    // short-lived Copilot token). `validate()` guarantees `derived` is set
    // for device-flow providers that need it.
    let mut token = OAuthToken {
        access_token: github_token,
        refresh_token: None,
        expires_at: None, // GitHub OAuth tokens from the device flow do not expire
        extra: Default::default(),
    };
    if let Some(derived) = spec.derived.as_ref() {
        let (derived_token, derived_expires) =
            exchange_derived_token(derived, &token.access_token).await?;
        set_derived_extra(&mut token, &derived_token, derived_expires);
    }
    Ok(token)
}

/// GET the derived API token described by the spec (e.g. GitHub OAuth
/// token → Copilot API token).
async fn exchange_derived_token(
    derived: &DerivedCredential,
    access_token: &str,
) -> Result<(String, DateTime<Utc>)> {
    let mut req = http_client()?
        .get(&derived.url)
        .header("Accept", "application/json")
        .header(
            "Authorization",
            format!("{} {access_token}", derived.auth_scheme),
        );
    for (k, v) in &derived.headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req.send().await?;
    let status = resp.status();
    let json: Value = resp.json().await?;
    if !status.is_success() {
        let msg = json["message"].as_str().unwrap_or("unknown error");
        return Err(AuthError::TokenExchange(format!(
            "derived-token exchange failed ({status}): {msg}"
        )));
    }
    let token = json["token"]
        .as_str()
        .ok_or_else(|| AuthError::TokenExchange("derived exchange: missing token".to_string()))?
        .to_string();
    let expires_at = json["expires_at"]
        .as_i64()
        .and_then(|secs| DateTime::from_timestamp(secs, 0))
        .ok_or_else(|| {
            AuthError::TokenExchange("derived exchange: missing expires_at".to_string())
        })?;
    Ok((token, expires_at))
}

fn set_derived_extra(token: &mut OAuthToken, derived_token: &str, expires: DateTime<Utc>) {
    token.extra.insert(
        "derived_token".to_string(),
        Value::String(derived_token.to_string()),
    );
    token.extra.insert(
        "derived_expires_at".to_string(),
        Value::String(expires.to_rfc3339()),
    );
}

/// The derived API token lives in `extra`; re-exchange when it is near
/// expiry. The underlying OAuth token itself does not expire (GitHub
/// device-flow tokens).
async fn ensure_fresh_derived(
    spec: &ProviderSpec,
    store: &TokenStore,
    method: &str,
    token: OAuthToken,
) -> Result<OAuthToken> {
    // "copilot_*" keys are the pre-rename names; read them so stores
    // written by older builds keep working.
    let extra_get =
        |key: &str, legacy: &str| token.extra.get(key).or_else(|| token.extra.get(legacy));
    let fresh = extra_get("derived_expires_at", "copilot_expires_at")
        .and_then(Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|at| Utc::now() + chrono::Duration::seconds(60) < at.with_timezone(&Utc))
        .unwrap_or(false);
    if fresh && extra_get("derived_token", "copilot_token").is_some() {
        return Ok(token);
    }
    tracing::debug!(
        provider = spec.name,
        "derived API token expired; re-exchanging"
    );
    reexchange_derived(spec, store, method, token).await
}

/// Run the derived-token exchange and persist the result, preserving the
/// credential's import method. Shared by the expiry path and the forced
/// re-exchange after a 401.
async fn reexchange_derived(
    spec: &ProviderSpec,
    store: &TokenStore,
    method: &str,
    mut token: OAuthToken,
) -> Result<OAuthToken> {
    let derived = spec
        .derived
        .as_ref()
        .ok_or_else(|| AuthError::Provider(format!("{}: no derived credential spec", spec.name)))?;
    let (derived_token, expires) = exchange_derived_token(derived, &token.access_token).await?;
    set_derived_extra(&mut token, &derived_token, expires);
    persist_token(store, spec.name.as_str(), method, &token)?;
    Ok(token)
}

fn persist_token(
    store: &TokenStore,
    provider: &str,
    method: &str,
    token: &OAuthToken,
) -> Result<()> {
    let auth = ProviderAuth {
        method: method.to_string(),
        token: token.clone(),
    };
    store.set(provider, auth)
}

// ────────────────────────────────────────────────────────────────────────
// Token endpoint helpers
// ────────────────────────────────────────────────────────────────────────

/// Grant payload ready to send — kept pure so the conformance tests can
/// assert per-provider encodings without any network.
enum GrantBody {
    Json(Value),
    Form(Vec<(&'static str, String)>),
}

/// Authorization-code exchange (PKCE) body, encoded per `spec.token_encoding`.
fn code_exchange_body(
    spec: &ProviderSpec,
    code: &str,
    redirect_uri: &str,
    pkce: &Pkce,
) -> GrantBody {
    match spec.token_encoding {
        TokenEncoding::Json => GrantBody::Json(serde_json::json!({
            "grant_type": "authorization_code",
            "client_id": spec.client_id,
            "code": code,
            "state": pkce.state,
            "redirect_uri": redirect_uri,
            "code_verifier": pkce.verifier,
        })),
        TokenEncoding::Form => {
            let mut form: Vec<(&'static str, String)> = vec![
                ("grant_type", "authorization_code".to_string()),
                ("client_id", spec.client_id.to_string()),
                ("code", code.to_string()),
                ("redirect_uri", redirect_uri.to_string()),
                ("code_verifier", pkce.verifier.clone()),
            ];
            if let Some(secret) = spec.client_secret.as_deref() {
                form.push(("client_secret", secret.to_string()));
            }
            GrantBody::Form(form)
        }
    }
}

/// Refresh-token grant body, encoded per `spec.token_encoding`.
fn refresh_body(spec: &ProviderSpec, refresh_token: &str) -> GrantBody {
    match spec.token_encoding {
        TokenEncoding::Json => GrantBody::Json(serde_json::json!({
            "grant_type": "refresh_token",
            "client_id": spec.client_id,
            "refresh_token": refresh_token,
        })),
        TokenEncoding::Form => {
            let mut form: Vec<(&'static str, String)> = vec![
                ("grant_type", "refresh_token".to_string()),
                ("client_id", spec.client_id.to_string()),
                ("refresh_token", refresh_token.to_string()),
            ];
            if let Some(secret) = spec.client_secret.as_deref() {
                form.push(("client_secret", secret.to_string()));
            }
            GrantBody::Form(form)
        }
    }
}

async fn send_grant(spec: &ProviderSpec, body: GrantBody) -> Result<OAuthToken> {
    let client = http_client()?;
    let req = client.post(&spec.token_url);
    let resp = match body {
        GrantBody::Json(json) => req.json(&json).send().await?,
        GrantBody::Form(form) => {
            req.header("Accept", "application/json")
                .form(&form)
                .send()
                .await?
        }
    };
    parse_token_response(resp).await
}

/// Authorization-code exchange (PKCE).
async fn exchange_code(
    spec: &ProviderSpec,
    code: &str,
    redirect_uri: &str,
    pkce: &Pkce,
) -> Result<OAuthToken> {
    send_grant(spec, code_exchange_body(spec, code, redirect_uri, pkce)).await
}

/// Refresh-token grant for providers that issue refresh tokens.
async fn refresh_grant(spec: &ProviderSpec, refresh_token: &str) -> Result<OAuthToken> {
    send_grant(spec, refresh_body(spec, refresh_token)).await
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
/// for the Codex backend route, which needs it as a header. `pub(crate)`
/// for credential import (Codex `auth.json` may lack `account_id`).
pub(crate) fn openai_account_id_from_jwt(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let json: Value = serde_json::from_slice(&bytes).ok()?;
    json["https://api.openai.com/auth"]["chatgpt_account_id"]
        .as_str()
        .map(str::to_string)
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(concat!("whycodes/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(AuthError::Http)
}

pub use crate::spec::{spec_for, suggested_models, supports_oauth, validate};

#[cfg(test)]
#[path = "providers_tests.rs"]
mod tests;
