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

/// Mark that `provider`'s current credential came from the OAuth token
/// store under `data_dir`, so a 401 may trigger one forced refresh.
pub fn register(provider: &str, data_dir: PathBuf) {
    if let Ok(mut map) = sources().write() {
        map.insert(provider.to_string(), data_dir);
    }
}

/// Drop the registration — an explicit API key replaced the OAuth token
/// (or the user logged out), so a 401 must surface without a retry.
pub fn unregister(provider: &str) {
    if let Ok(mut map) = sources().write() {
        map.remove(provider);
    }
}

/// True when `provider` has a registered OAuth credential source.
pub fn has_source(provider: &str) -> bool {
    sources()
        .read()
        .map(|map| map.contains_key(provider))
        .unwrap_or(false)
}

fn source_dir(provider: &str) -> Option<PathBuf> {
    sources().read().ok()?.get(provider).cloned()
}

/// Read a provider-specific extra stored with the OAuth credential (e.g.
/// `openai_account_id`, sent as the `chatgpt-account-id` header on the
/// Codex backend). `None` when no source is registered or the key is
/// absent. Never returns token material — callers must name a non-secret
/// extra key.
pub async fn stored_extra(provider: &str, key: &str) -> Option<String> {
    let dir = source_dir(provider)?;
    let store = whycodes_auth::TokenStore::new(&dir);
    let auth = store.get(provider).ok()??;
    auth.token.extra.get(key)?.as_str().map(str::to_string)
}

/// Send the request built by `build(current_key)`; on a 401 with a
/// registered OAuth source, force-refresh the stored token and send
/// `build(new_key)` once. `build` must be cheap to call twice (the body is
/// borrowed, not consumed).
pub async fn send_with_refresh_retry(
    provider: &str,
    current_key: &str,
    build: impl Fn(&str) -> reqwest::RequestBuilder,
) -> whycodes_core::Result<reqwest::Response> {
    let resp = build(current_key)
        .send()
        .await
        .map_err(|e| whycodes_core::Error::llm(format!("HTTP error: {e}")))?;
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
    tracing::info!(
        provider,
        "401 with OAuth credential; token renewed, retrying request once"
    );
    build(&fresh)
        .send()
        .await
        .map_err(|e| whycodes_core::Error::llm(format!("HTTP error: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_unregister_roundtrip() {
        let dir = PathBuf::from("/tmp/whycodes-test-oauth-src");
        assert!(!has_source("test-provider"));
        register("test-provider", dir.clone());
        assert!(has_source("test-provider"));
        assert_eq!(source_dir("test-provider"), Some(dir));
        unregister("test-provider");
        assert!(!has_source("test-provider"));
        assert_eq!(source_dir("test-provider"), None);
    }

    #[test]
    fn sources_are_per_provider() {
        let dir = PathBuf::from("/tmp/whycodes-test-oauth-src-2");
        register("prov-a", dir.clone());
        assert!(has_source("prov-a"));
        assert!(!has_source("prov-b"));
        unregister("prov-a");
    }

    fn serve_status(status: &str, body: &str) -> String {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let header = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let payload = format!("{header}{body}");
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(payload.as_bytes());
            }
        });
        format!("http://{addr}/")
    }

    #[tokio::test]
    async fn send_returns_success_and_401_without_refresh_source() {
        let ok_url = serve_status("200 OK", "ok");
        let ok = send_with_refresh_retry("no-such-provider", "token", |key| {
            crate::client_identity::http_client()
                .post(&ok_url)
                .bearer_auth(key)
        })
        .await
        .unwrap();
        assert_eq!(ok.status().as_u16(), 200);

        let err_url = serve_status("401 Unauthorized", "nope");
        let err = send_with_refresh_retry("no-such-provider", "token", |key| {
            crate::client_identity::http_client()
                .post(&err_url)
                .bearer_auth(key)
        })
        .await
        .unwrap();
        assert_eq!(err.status().as_u16(), 401);
        assert_eq!(err.text().await.unwrap(), "nope");
    }
}
