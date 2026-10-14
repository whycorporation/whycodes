//! OAuth for remote MCP servers, per the MCP authorization spec:
//! protected-resource metadata (RFC 9728), authorization server metadata
//! (RFC 8414), dynamic client registration (RFC 7591), and the
//! authorization code flow with PKCE and a `resource` indicator (RFC 8707).
//!
//! Tokens live in `<data_dir>/mcp/auth.json` under `mcp:<server>`, apart
//! from provider logins.

use std::path::Path;
use std::time::Duration;

use reqwest::header::{ACCEPT, CONTENT_TYPE, WWW_AUTHENTICATE};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use whycodes_auth::flow::{authorize_url, bind_loopback, wait_for_callback};
use whycodes_auth::pkce::Pkce;
use whycodes_auth::{AuthError, OAuthToken, ProviderAuth, TokenStore};

use crate::error::{McpError, Result};

/// How long `login` waits for the browser redirect.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

/// What a server's authorization server publishes (RFC 8414).
#[derive(Debug, Clone, Deserialize)]
pub struct AuthServerMetadata {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(default)]
    pub registration_endpoint: Option<String>,
    #[serde(default)]
    pub scopes_supported: Vec<String>,
}

#[derive(Deserialize)]
struct ResourceMetadata {
    #[serde(default)]
    resource: Option<String>,
    #[serde(default)]
    authorization_servers: Vec<String>,
    #[serde(default)]
    scopes_supported: Vec<String>,
}

/// Where and how to authorize for one MCP server.
#[derive(Debug, Clone)]
pub struct Discovery {
    /// Resource indicator sent with the authorization and token requests.
    pub resource: String,
    pub metadata: AuthServerMetadata,
    pub scopes: Vec<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

fn store(data_dir: &Path) -> TokenStore {
    TokenStore::new(&data_dir.join("mcp"))
}

fn key(server: &str) -> String {
    format!("mcp:{server}")
}

fn auth_err(e: AuthError) -> McpError {
    McpError::msg(&e.to_string())
}

/// `resource_metadata` from a `WWW-Authenticate: Bearer …` challenge.
pub(crate) fn resource_metadata_url(header: &str) -> Option<String> {
    let start = header.find("resource_metadata=")? + "resource_metadata=".len();
    let rest = &header[start..];
    let value = match rest.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next().unwrap_or(""),
        None => rest.split([',', ' ']).next().unwrap_or(""),
    };
    (!value.is_empty()).then(|| value.to_string())
}

fn origin_and_path(url: &str) -> Result<(String, String)> {
    let parsed =
        url::Url::parse(url).map_err(|e| McpError::msg(&format!("invalid URL {url}: {e}")))?;
    let path = parsed.path().trim_end_matches('/').to_string();
    Ok((parsed.origin().ascii_serialization(), path))
}

/// GET `url` as JSON; `None` on any failure (the caller tries the next path).
async fn get_json(http: &reqwest::Client, url: &str) -> Option<Value> {
    let response = http
        .get(url)
        .header(ACCEPT, "application/json")
        .send()
        .await;
    match response {
        Ok(r) if r.status().is_success() => r.json().await.ok(),
        Ok(r) => {
            tracing::debug!(%url, status = %r.status(), "MCP OAuth metadata not here");
            None
        }
        Err(e) => {
            tracing::debug!(%url, error = %e, "MCP OAuth metadata fetch failed");
            None
        }
    }
}

/// Find the authorization server for `server_url`: the 401 challenge's
/// `resource_metadata`, then the well-known protected-resource paths, and
/// finally the server's own origin (servers from before RFC 9728).
pub async fn discover(http: &reqwest::Client, server_url: &str) -> Result<Discovery> {
    let (origin, path) = origin_and_path(server_url)?;
    let probe = http
        .post(server_url)
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .body(r#"{"jsonrpc":"2.0","id":0,"method":"ping"}"#)
        .send()
        .await;
    let hinted = probe
        .as_ref()
        .ok()
        .and_then(|r| r.headers().get(WWW_AUTHENTICATE))
        .and_then(|v| v.to_str().ok())
        .and_then(resource_metadata_url);
    let mut candidates: Vec<String> = hinted.into_iter().collect();
    candidates.push(format!(
        "{origin}/.well-known/oauth-protected-resource{path}"
    ));
    candidates.push(format!("{origin}/.well-known/oauth-protected-resource"));
    candidates.dedup();
    let mut found = None;
    for url in &candidates {
        let Some(meta) = get_json(http, url).await else {
            continue;
        };
        match serde_json::from_value::<ResourceMetadata>(meta) {
            Ok(meta) => {
                found = Some(meta);
                break;
            }
            Err(e) => tracing::debug!(%url, error = %e, "bad protected-resource metadata"),
        }
    }
    let (issuer, resource, scopes) = match found {
        Some(meta) => (
            meta.authorization_servers
                .first()
                .cloned()
                .unwrap_or_else(|| origin.clone()),
            meta.resource.unwrap_or_else(|| server_url.to_string()),
            meta.scopes_supported,
        ),
        None => (origin, server_url.to_string(), Vec::new()),
    };
    let metadata = auth_server_metadata(http, &issuer).await?;
    let scopes = if scopes.is_empty() {
        metadata.scopes_supported.clone()
    } else {
        scopes
    };
    Ok(Discovery {
        resource,
        metadata,
        scopes,
    })
}

async fn auth_server_metadata(http: &reqwest::Client, issuer: &str) -> Result<AuthServerMetadata> {
    let (origin, path) = origin_and_path(issuer)?;
    let mut candidates = vec![
        format!("{origin}/.well-known/oauth-authorization-server{path}"),
        format!("{origin}/.well-known/openid-configuration{path}"),
        format!("{origin}{path}/.well-known/openid-configuration"),
    ];
    candidates.dedup();
    for url in &candidates {
        let Some(meta) = get_json(http, url).await else {
            continue;
        };
        match serde_json::from_value::<AuthServerMetadata>(meta) {
            Ok(meta) => return Ok(meta),
            Err(e) => tracing::debug!(%url, error = %e, "bad authorization server metadata"),
        }
    }
    Err(McpError::msg(&format!(
        "no OAuth authorization server metadata for {issuer}"
    )))
}

/// Register WhyCodes as a public client; `(client_id, client_secret)`.
async fn register_client(
    http: &reqwest::Client,
    endpoint: &str,
    redirect_uri: &str,
) -> Result<(String, Option<String>)> {
    let body = json!({
        "client_name": "WhyCodes",
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    let response = http
        .post(endpoint)
        .json(&body)
        .send()
        .await
        .map_err(|e| McpError::msg(&format!("client registration failed: {e}")))?;
    let status = response.status();
    let value: Value = response.json().await.unwrap_or(Value::Null);
    let client_id = value.get("client_id").and_then(Value::as_str);
    match client_id {
        Some(id) if status.is_success() => Ok((
            id.to_string(),
            value
                .get("client_secret")
                .and_then(Value::as_str)
                .map(str::to_string),
        )),
        _ => Err(McpError::msg(&format!(
            "client registration returned {status}: {value}"
        ))),
    }
}

async fn token_request(
    http: &reqwest::Client,
    endpoint: &str,
    form: &[(&str, &str)],
) -> Result<TokenResponse> {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form)
        .finish();
    let response = http
        .post(endpoint)
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(ACCEPT, "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| McpError::msg(&format!("token request failed: {e}")))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(McpError::msg(&format!(
            "token endpoint returned {status}: {text}"
        )));
    }
    serde_json::from_str(&text).map_err(|e| McpError::msg(&format!("invalid token response: {e}")))
}

fn to_token(
    response: TokenResponse,
    previous_refresh: Option<String>,
    extra: Map<String, Value>,
) -> OAuthToken {
    OAuthToken {
        access_token: response.access_token,
        refresh_token: response.refresh_token.or(previous_refresh),
        expires_at: response
            .expires_in
            .map(|s| chrono::Utc::now() + chrono::Duration::seconds(s)),
        extra,
    }
}

/// Browser login for `server`. `open` gets the authorization URL (open a
/// browser, print it); the redirect lands on a loopback listener.
pub async fn login(
    server: &str,
    server_url: &str,
    data_dir: &Path,
    open: &(dyn Fn(&str) + Sync),
) -> Result<()> {
    let http = reqwest::Client::new();
    let found = discover(&http, server_url).await?;
    let registration = found
        .metadata
        .registration_endpoint
        .as_deref()
        .ok_or_else(|| {
            McpError::msg(
                "the authorization server does not offer dynamic client registration; \
             set an Authorization header for this server in config instead",
            )
        })?;
    let (listener, port) = bind_loopback().map_err(auth_err)?;
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let (client_id, client_secret) = register_client(&http, registration, &redirect_uri).await?;
    let pkce = Pkce::new();
    let url = authorize_url(
        &found.metadata.authorization_endpoint,
        &client_id,
        &redirect_uri,
        &found.scopes.join(" "),
        &pkce,
        &[("resource".to_string(), found.resource.clone())],
    );
    open(&url);
    let state = pkce.state.clone();
    let callback = callback_result(
        tokio::task::spawn_blocking(move || wait_for_callback(&listener, &state, CALLBACK_TIMEOUT))
            .await,
    )?;
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", callback.code.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("client_id", client_id.as_str()),
        ("code_verifier", pkce.verifier.as_str()),
        ("resource", found.resource.as_str()),
    ];
    if let Some(secret) = &client_secret {
        form.push(("client_secret", secret));
    }
    let response = token_request(&http, &found.metadata.token_endpoint, &form).await?;
    let mut extra = Map::new();
    extra.insert(
        "token_endpoint".into(),
        found.metadata.token_endpoint.into(),
    );
    extra.insert("client_id".into(), client_id.into());
    extra.insert("resource".into(), found.resource.into());
    if let Some(secret) = client_secret {
        extra.insert("client_secret".into(), secret.into());
    }
    let auth = ProviderAuth {
        method: "oauth".into(),
        token: to_token(response, None, extra),
    };
    store(data_dir).set(&key(server), auth).map_err(auth_err)
}

fn callback_result(
    joined: std::result::Result<
        std::result::Result<whycodes_auth::flow::CallbackResult, AuthError>,
        tokio::task::JoinError,
    >,
) -> Result<whycodes_auth::flow::CallbackResult> {
    match joined {
        Ok(result) => result.map_err(auth_err),
        Err(e) => Err(McpError::msg(&format!("OAuth callback wait failed: {e}"))),
    }
}

/// The stored access token for `server`, refreshed first when it expired.
/// `None` when there is no usable token.
pub async fn bearer(server: &str, data_dir: &Path) -> Option<String> {
    let store = store(data_dir);
    let auth = match store.get(&key(server)) {
        Ok(found) => found?,
        Err(e) => {
            tracing::warn!(%server, error = %e, "MCP token store unreadable");
            return None;
        }
    };
    if !auth.token.is_expired() {
        return Some(auth.token.access_token);
    }
    match refresh(&auth.token).await {
        Ok(token) => {
            let access = token.access_token.clone();
            let renewed = ProviderAuth {
                method: auth.method,
                token,
            };
            if let Err(e) = store.set(&key(server), renewed) {
                tracing::warn!(%server, error = %e, "MCP token save failed");
            }
            Some(access)
        }
        Err(e) => {
            tracing::warn!(%server, error = %e, "MCP token expired; run `whycodes mcp login`");
            None
        }
    }
}

async fn refresh(token: &OAuthToken) -> Result<OAuthToken> {
    let field = |k: &str| token.extra.get(k).and_then(Value::as_str);
    let (Some(refresh), Some(endpoint), Some(client_id)) = (
        token.refresh_token.as_deref(),
        field("token_endpoint"),
        field("client_id"),
    ) else {
        return Err(McpError::msg("token cannot be refreshed"));
    };
    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh),
        ("client_id", client_id),
    ];
    for name in ["resource", "client_secret"] {
        if let Some(value) = field(name) {
            form.push((name, value));
        }
    }
    let response = token_request(&reqwest::Client::new(), endpoint, &form).await?;
    Ok(to_token(
        response,
        Some(refresh.to_string()),
        token.extra.clone(),
    ))
}

/// Forget `server`'s token; `true` when one was stored.
pub fn logout(server: &str, data_dir: &Path) -> Result<bool> {
    store(data_dir).remove(&key(server)).map_err(auth_err)
}

#[cfg(test)]
#[path = "oauth_tests.rs"]
mod tests;
