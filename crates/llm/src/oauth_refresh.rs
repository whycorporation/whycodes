//! One-shot credential renewal when a provider answers 401 on an OAuth
//! subscription token the store considered fresh.
//!
//! Call sites that resolve a credential from the OAuth store
//! (`whycodes auth login <provider>`) register the provider here; call sites
//! that resolve an explicit API key (env var / config) unregister it.
//! Providers that send OAuth bearer tokens route their POST through
//! [`send_with_refresh_retry`]: on a 401 they force-refresh the stored
//! token once and retry the request exactly once. Every other case — no
//! registered source, refresh refused, second 401 — returns the response
//! untouched so the normal error path reports it.
//!
//! This is deliberately *not* part of the generic retry policy
//! (`retry.rs` keeps 401 non-retryable). Only a request carrying an OAuth
//! credential may retry, only after renewal, and only once.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

/// provider name → data dir holding the OAuth token store.
fn sources() -> &'static RwLock<HashMap<String, PathBuf>> {
    static SOURCES: OnceLock<RwLock<HashMap<String, PathBuf>>> = OnceLock::new();
    SOURCES.get_or_init(|| RwLock::new(HashMap::new()))
}

fn write_sources() -> std::sync::RwLockWriteGuard<'static, HashMap<String, PathBuf>> {
    sources().write().unwrap_or_else(|e| e.into_inner())
}

fn read_sources() -> std::sync::RwLockReadGuard<'static, HashMap<String, PathBuf>> {
    sources().read().unwrap_or_else(|e| e.into_inner())
}

/// Mark that `provider`'s current credential came from the OAuth token
/// store under `data_dir`, so a 401 may trigger one forced refresh.
pub fn register(provider: &str, data_dir: PathBuf) {
    write_sources().insert(provider.to_string(), data_dir);
}

/// Drop the registration — an explicit API key replaced the OAuth token
/// (or the user logged out), so a 401 must surface without a retry.
pub fn unregister(provider: &str) {
    write_sources().remove(provider);
}

/// True when `provider` has a registered OAuth credential source.
pub fn has_source(provider: &str) -> bool {
    read_sources().contains_key(provider)
}

fn source_dir(provider: &str) -> Option<PathBuf> {
    read_sources().get(provider).cloned()
}

/// Read a provider-specific extra stored with the OAuth credential (e.g.
/// `openai_account_id`, sent as the `chatgpt-account-id` header on the
/// Codex backend). `None` when no source is registered or the key is
/// absent. Never returns token material — callers must name a non-secret
/// extra key.
pub async fn stored_extra(provider: &str, key: &str) -> Option<String> {
    let dir = source_dir(provider)?;
    let store = whycodes_auth::TokenStore::new(&dir);
    auth_from_store(store.get(provider))?
        .token
        .extra
        .get(key)?
        .as_str()
        .map(str::to_string)
}

fn auth_from_store(
    got: Result<Option<whycodes_auth::ProviderAuth>, whycodes_auth::AuthError>,
) -> Option<whycodes_auth::ProviderAuth> {
    match got {
        Ok(Some(auth)) => Some(auth),
        Ok(None) => None,
        Err(_store) => None,
    }
}

#[cfg(test)]
pub(crate) fn auth_from_store_err_for_tests() -> Option<whycodes_auth::ProviderAuth> {
    auth_from_store(Err(whycodes_auth::AuthError::Json(
        serde_json::from_str::<serde_json::Value>("{").unwrap_err(),
    )))
}

#[cfg(test)]
pub(crate) fn poison_sources_for_tests() {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = sources().write().unwrap();
        panic!("poison oauth sources");
    }));
}

/// Send the request built by `build(current_key)`; on a 401 with a
/// registered OAuth source, force-refresh the stored token and send
/// `build(new_key)` once. `build` must be cheap to call twice (the body is
/// borrowed, not consumed).
pub async fn send_with_refresh_retry(
    provider: &str,
    current_key: &str,
    build: impl Fn(&str) -> reqwest::RequestBuilder + Send + Sync,
) -> whycodes_core::Result<reqwest::Response> {
    send_with_refresh_retry_dyn(provider, current_key, &build).await
}

async fn send_with_refresh_retry_dyn(
    provider: &str,
    current_key: &str,
    build: &(dyn Fn(&str) -> reqwest::RequestBuilder + Send + Sync),
) -> whycodes_core::Result<reqwest::Response> {
    let resp = match build(current_key).send().await {
        Ok(resp) => resp,
        Err(err) => return Err(http_error(&err.to_string())),
    };
    if resp.status() != reqwest::StatusCode::UNAUTHORIZED {
        return Ok(resp);
    }
    let Some(dir) = source_dir(provider) else {
        return Ok(resp);
    };
    let Some(fresh) = whycodes_auth::providers::force_refresh(provider, &dir).await else {
        return Ok(resp);
    };
    if fresh == current_key {
        // Renewal produced the same credential — retrying would loop on
        // the same 401. Hand the original response to the error path.
        return Ok(resp);
    }
    let _ = provider;
    match build(&fresh).send().await {
        Ok(resp) => Ok(resp),
        Err(err) => Err(http_error(&err.to_string())),
    }
}

fn http_error(err: &str) -> whycodes_core::Error {
    let mut msg = String::from("HTTP error: ");
    msg.push_str(err);
    whycodes_core::Error::llm(msg)
}

#[cfg(test)]
pub(crate) fn http_error_for_tests(err: &str) -> whycodes_core::Error {
    http_error(err)
}

#[cfg(test)]
#[path = "oauth_refresh_tests.rs"]
mod tests;
