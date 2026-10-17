//! Loopback discovery of OpenAI-compatible local model servers.
//!
//! Each candidate is probed with `GET http://127.0.0.1:<port>/v1/models`
//! under one short timeout. A 2xx JSON reply whose `data` field is an array
//! counts as a running server; anything else (refused, timeout, non-JSON,
//! no `data` array) is "not running" and only logged at debug level.
//!
//! Probing runs only on demand (`whycodes provider discover`, the TUI
//! provider dialog) — never at startup.

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use whycodes_core::types::ProviderConfig;

/// A well-known local server: the provider id it registers as, its display
/// label and the port it listens on by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalServerKind {
    /// Provider id written to `[providers.<id>]`.
    pub provider: &'static str,
    /// Human label for listings.
    pub label: &'static str,
    /// Default loopback port.
    pub port: u16,
}

/// Servers probed by [`discover_local_servers`], in listing order.
///
/// Ollama is not probed: it already has its own native provider.
pub const LOCAL_SERVER_CANDIDATES: &[LocalServerKind] = &[
    LocalServerKind {
        provider: "lmstudio",
        label: "LM Studio",
        port: 1234,
    },
    LocalServerKind {
        provider: "vllm",
        label: "vLLM",
        port: 8000,
    },
    LocalServerKind {
        provider: "llamacpp",
        label: "llama.cpp server",
        port: 8080,
    },
];

/// Connect + response budget per probe.
pub const DEFAULT_DISCOVERY_TIMEOUT: Duration = Duration::from_millis(250);

/// A local server that answered `/v1/models`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalServer {
    /// Provider id (`lmstudio`, `vllm`, `llamacpp`).
    pub provider: String,
    /// Display label (`LM Studio`, …).
    pub label: String,
    /// OpenAI-compatible base, e.g. `http://127.0.0.1:1234/v1`.
    pub api_base: String,
    /// Model ids from the `data` array (may be empty when nothing is loaded).
    pub models: Vec<String>,
}

impl LocalServer {
    /// The `[providers.<id>]` block that makes this server usable: the
    /// OpenAI-compatible base as `api_base`, no key, the listed models.
    pub fn provider_config(&self) -> ProviderConfig {
        ProviderConfig {
            name: self.provider.clone(),
            api_key: None,
            api_base: Some(self.api_base.clone()),
            base_url: None,
            headers: None,
            models: self.models.clone(),
            tool_arguments: None,
            extra: Default::default(),
            credentials: Default::default(),
        }
    }
}

/// What saving a discovered server does to an existing `[providers.<id>]`.
#[derive(Debug, Clone)]
pub enum LocalSave {
    /// No block yet: write this one.
    New(ProviderConfig),
    /// A block without `api_base` / `base_url`: the same block with the
    /// discovered `api_base` filled in; every other field is kept.
    FillApiBase(ProviderConfig),
    /// A block already pointing at this server: nothing to write.
    AlreadyConfigured,
    /// A block pointing somewhere else: left untouched, nothing written.
    Conflict {
        /// The base the user configured (`base_url`, else `api_base`).
        configured: String,
    },
}

impl LocalServer {
    /// Decide how to save this server next to `existing` without ever
    /// overwriting a user-configured block.
    pub fn plan_save(&self, existing: Option<&ProviderConfig>) -> LocalSave {
        let Some(existing) = existing else {
            return LocalSave::New(self.provider_config());
        };
        let configured = existing.base_url.as_ref().or(existing.api_base.as_ref());
        match configured {
            None => {
                let mut filled = existing.clone();
                filled.api_base = Some(self.api_base.clone());
                LocalSave::FillApiBase(filled)
            }
            Some(base) if same_base(base, &self.api_base) => LocalSave::AlreadyConfigured,
            Some(base) => LocalSave::Conflict {
                configured: base.clone(),
            },
        }
    }
}

/// Bases equal up to trailing slashes and ASCII case.
fn same_base(a: &str, b: &str) -> bool {
    a.trim_end_matches('/')
        .eq_ignore_ascii_case(b.trim_end_matches('/'))
}

/// Loopback `/v1` base for a candidate port.
pub fn loopback_api_base(port: u16) -> String {
    format!("http://127.0.0.1:{port}/v1")
}

/// Probe every entry of [`LOCAL_SERVER_CANDIDATES`] concurrently.
pub async fn discover_local_servers(timeout: Duration) -> Vec<LocalServer> {
    let candidates: Vec<(LocalServerKind, String)> = LOCAL_SERVER_CANDIDATES
        .iter()
        .map(|kind| (*kind, loopback_api_base(kind.port)))
        .collect();
    discover_servers_at(&candidates, timeout).await
}

/// Probe `(kind, api_base)` pairs concurrently; found servers keep input order.
pub async fn discover_servers_at(
    candidates: &[(LocalServerKind, String)],
    timeout: Duration,
) -> Vec<LocalServer> {
    let probes = candidates.iter().map(|(kind, api_base)| async move {
        probe_openai_models(api_base, timeout)
            .await
            .map(|models| LocalServer {
                provider: kind.provider.to_string(),
                label: kind.label.to_string(),
                api_base: api_base.clone(),
                models,
            })
    });
    futures::future::join_all(probes)
        .await
        .into_iter()
        .flatten()
        .collect()
}

/// `GET {api_base}/models`; `Some(model ids)` only for a 2xx JSON body with a
/// `data` array. Every failure is debug-logged and returns `None`.
pub async fn probe_openai_models(api_base: &str, timeout: Duration) -> Option<Vec<String>> {
    let url = super::model_catalog::normalize_models_url(api_base);
    let response = super::client_identity::http_client()
        .get(&url)
        .timeout(timeout)
        .send()
        .await;
    let response = match response {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!(url, "local server not running: {e}");
            return None;
        }
    };
    let status = response.status();
    if !status.is_success() {
        tracing::debug!(url, %status, "local server probe: non-success status");
        return None;
    }
    let json: Value = match response.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(url, "local server probe: not JSON: {e}");
            return None;
        }
    };
    let Some(data) = json.get("data").and_then(Value::as_array) else {
        tracing::debug!(url, "local server probe: no `data` array");
        return None;
    };
    Some(
        data.iter()
            .filter_map(|m| m.get("id").and_then(Value::as_str))
            .map(str::to_string)
            .collect(),
    )
}

#[cfg(test)]
#[path = "local_discovery_tests.rs"]
mod tests;
