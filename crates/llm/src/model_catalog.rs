//! OpenAI-compatible `GET /v1/models` catalog.
//!
//! Gateways (OmniRoute, LiteLLM, OpenRouter, vLLM, …) often expose per-model
//! `context_length` / `max_input_tokens` here — that is the authoritative max
//! for the meter, not chat-response headers.

use serde_json::Value;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Parsed snapshot of a provider's model list.
#[derive(Debug, Clone, Default)]
pub struct ModelCatalog {
    /// Model id → context window tokens.
    pub context_windows: HashMap<String, u32>,
    /// Model id → max completion tokens (when the API reported it).
    pub max_output_tokens: HashMap<String, u32>,
    /// When this snapshot was fetched (for TTL / refresh).
    pub fetched_at: Option<Instant>,
    /// Source URL (debug).
    pub source_url: String,
}

impl ModelCatalog {
    pub fn context_window(&self, model: &str) -> Option<u32> {
        self.context_windows.get(model).copied().or_else(|| {
            // Some gateways return `provider/model`; callers may only have `model`.
            self.context_windows
                .iter()
                .find(|(id, _)| id.ends_with(&format!("/{model}")) || *id == model)
                .map(|(_, n)| *n)
        })
    }

    pub fn is_empty(&self) -> bool {
        self.context_windows.is_empty()
    }

    pub fn is_stale(&self, ttl: Duration) -> bool {
        match self.fetched_at {
            Some(t) => t.elapsed() > ttl,
            None => true,
        }
    }
}

/// Default cache lifetime for a catalog snapshot.
pub const CATALOG_TTL: Duration = Duration::from_secs(15 * 60);

/// Turn a chat/completions base into `…/v1/models`.
///
/// Accepts:
/// - `http://host:port/v1`
/// - `http://host:port/v1/chat/completions`
/// - `http://host:port/v1/models` (idempotent)
pub fn normalize_models_url(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    if base.ends_with("/models") {
        return base.to_string();
    }
    if let Some(prefix) = base.strip_suffix("/chat/completions") {
        return format!("{}/models", prefix.trim_end_matches('/'));
    }
    // Bare `/v1` or host root → append `/models` (or `/v1/models` if no /v1).
    if base.ends_with("/v1") {
        format!("{base}/models")
    } else if base.contains("/v1/") {
        // e.g. unexpected path — still append models next to last segment's parent
        format!("{base}/models")
    } else {
        format!("{base}/v1/models")
    }
}

/// Extract context window from a single model object (OpenAI + common extensions).
pub fn context_window_from_model_value(m: &Value) -> Option<u32> {
    // Prefer full context, then input-only caps.
    const KEYS: &[&str] = &[
        "context_length",
        "context_window",
        "max_model_len", // vLLM
        "max_input_tokens",
        "max_tokens", // ambiguous; last resort
    ];
    for key in KEYS {
        if let Some(n) = as_u32(m.get(*key))
            && n > 0
        {
            return Some(n);
        }
    }
    // Nested OpenRouter-style: top_provider.context_length / architecture
    if let Some(tp) = m.get("top_provider")
        && let Some(n) = as_u32(tp.get("context_length")).filter(|n| *n > 0)
    {
        return Some(n);
    }
    if let Some(arch) = m.get("architecture")
        && let Some(n) = as_u32(arch.get("context_length")).filter(|n| *n > 0)
    {
        return Some(n);
    }
    None
}

fn as_u32(v: Option<&Value>) -> Option<u32> {
    let v = v?;
    if let Some(n) = v.as_u64()
        && let Ok(n) = u32::try_from(n)
    {
        return Some(n);
    }
    if let Some(n) = v.as_i64()
        && n > 0
        && let Ok(n) = u32::try_from(n)
    {
        return Some(n);
    }
    if let Some(f) = v.as_f64()
        && f > 0.0
        && f < u32::MAX as f64
    {
        return Some(f as u32);
    }
    if let Some(s) = v.as_str()
        && let Ok(n) = s.parse()
    {
        return Some(n);
    }
    None
}

/// Parse a `/v1/models` JSON body into a [`ModelCatalog`].
pub fn parse_models_json(json: &Value, source_url: &str) -> ModelCatalog {
    let mut catalog = ModelCatalog {
        source_url: source_url.to_string(),
        fetched_at: Some(Instant::now()),
        ..Default::default()
    };

    for m in model_items(json) {
        let id = m
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            continue;
        }
        if let Some(cw) = context_window_from_model_value(m) {
            catalog.context_windows.insert(id.clone(), cw);
        }
        if let Some(out) = as_u32(m.get("max_output_tokens"))
            .or_else(|| as_u32(m.get("max_completion_tokens")))
            .filter(|n| *n > 0)
        {
            catalog.max_output_tokens.insert(id, out);
        }
    }

    catalog
}

fn model_items(json: &Value) -> Vec<&Value> {
    if let Some(arr) = json.get("data").and_then(|d| d.as_array()) {
        arr.iter().collect()
    } else if let Some(arr) = json.as_array() {
        arr.iter().collect()
    } else {
        Vec::new()
    }
}

/// Look up a single model's context window without keeping the full catalog.
///
/// Prefer exact `id` match, then suffix `…/{model}`.
pub fn context_window_for_model_id(json: &Value, model: &str) -> Option<u32> {
    if model.is_empty() {
        return None;
    }
    let mut suffix_hit = None;
    for m in model_items(json) {
        let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if id.is_empty() {
            continue;
        }
        let Some(cw) = context_window_from_model_value(m) else {
            continue;
        };
        if id == model {
            return Some(cw);
        }
        if suffix_hit.is_none() && (id.ends_with(&format!("/{model}")) || id.ends_with(model)) {
            suffix_hit = Some(cw);
        }
    }
    suffix_hit
}

/// Inputs needed to call a provider's models list — all from config/runtime,
/// never hard-coded gateway hosts.
#[derive(Debug, Clone)]
pub struct CatalogFetchRequest {
    pub base_url: String,
    pub api_key: Option<String>,
    pub headers: HashMap<String, String>,
    pub provider_name: String,
}

/// Build a fetch request from config for `provider_name`.
///
/// Returns `None` when the provider has no `base_url` / `api_base` in config —
/// we do not invent endpoints. API key: config → `runtime_api_key` →
/// `{PROVIDER}_API_KEY` env. Headers come from `ProviderConfig.headers`
/// (may already include `Authorization` / `x-api-key`).
pub fn catalog_request_from_config(
    config: &whycode_config::Config,
    provider_name: &str,
    runtime_api_key: Option<&str>,
) -> Option<CatalogFetchRequest> {
    let pc = config.get_provider(provider_name)?;
    let base_url = base_url_from_provider_config(pc)?;
    let headers = pc.headers.clone().unwrap_or_default();

    let api_key = pc
        .api_key
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            runtime_api_key
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .or_else(|| {
            std::env::var(format!("{}_API_KEY", provider_name.to_uppercase()))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        });

    Some(CatalogFetchRequest {
        base_url,
        api_key,
        headers,
        provider_name: provider_name.to_string(),
    })
}

/// Fetch `GET {base}/models` using a config-derived request.
pub async fn fetch_model_catalog_from_request(
    req: &CatalogFetchRequest,
) -> whycode_core::Result<ModelCatalog> {
    fetch_model_catalog(&req.base_url, req.api_key.as_deref(), &req.headers).await
}

/// Fetch `/v1/models` and return **only** `model`'s context window (no full map on heap).
///
/// Prefer this on the TUI hot path: gateways can list thousands of models; we
/// only need the active one's `context_length`.
pub async fn fetch_model_context_window(
    req: &CatalogFetchRequest,
    model: &str,
) -> whycode_core::Result<Option<u32>> {
    let url = normalize_models_url(&req.base_url);
    let mut http =
        super::client_identity::with_identity(super::client_identity::http_client().get(&url));

    for (k, v) in &req.headers {
        http = http.header(k, v);
    }
    let has_authorization = req
        .headers
        .keys()
        .any(|k| k.eq_ignore_ascii_case("authorization"));
    if let Some(key) = req
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
        && !has_authorization
    {
        http = http.header("Authorization", format!("Bearer {key}"));
    }

    // Keep this short: a hung catalog on the same host as chat can queue the
    // first user turn on gateways with low concurrency (seen: 15s catalog
    // timeout → "Worked for 28s" while OmniRoute Duration was ~2s).
    let resp = http
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .map_err(|e| whycode_core::Error::Llm(format!("models list HTTP: {e}")))?;

    let status = resp.status();
    // Hard cap so a runaway gateway cannot OOM the agent (typical catalog ~1–2 MB).
    const MAX_BODY: usize = 8 * 1024 * 1024;
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| whycode_core::Error::Llm(format!("models list body: {e}")))?;
    if bytes.len() > MAX_BODY {
        return Err(whycode_core::Error::Llm(format!(
            "models list too large ({} bytes > {MAX_BODY})",
            bytes.len()
        )));
    }
    if !status.is_success() {
        let snippet: String = String::from_utf8_lossy(&bytes).chars().take(200).collect();
        return Err(whycode_core::Error::Llm(format!(
            "models list {status}: {snippet}"
        )));
    }

    let json: Value = serde_json::from_slice(&bytes)
        .map_err(|e| whycode_core::Error::Llm(format!("models list JSON: {e}")))?;
    Ok(context_window_for_model_id(&json, model))
}

/// Fetch `GET {base}/models` (OpenAI-compatible) and parse context windows.
///
/// Auth:
/// - `extra_headers` applied as-is (config may set `Authorization` or `x-api-key`)
/// - if no `Authorization` header, and `api_key` is set → `Bearer {api_key}`
/// - if no `x-api-key` header either, and key is set, we do **not** add
///   `x-api-key` automatically (only Bearer) — put it in config headers when needed
pub async fn fetch_model_catalog(
    base_url: &str,
    api_key: Option<&str>,
    extra_headers: &HashMap<String, String>,
) -> whycode_core::Result<ModelCatalog> {
    let url = normalize_models_url(base_url);
    let mut req =
        super::client_identity::with_identity(super::client_identity::http_client().get(&url));

    // Config headers first (may already carry Authorization / x-api-key).
    for (k, v) in extra_headers {
        req = req.header(k, v);
    }

    let has_authorization = extra_headers
        .keys()
        .any(|k| k.eq_ignore_ascii_case("authorization"));

    if let Some(key) = api_key.map(str::trim).filter(|k| !k.is_empty())
        && !has_authorization
    {
        req = req.header("Authorization", format!("Bearer {key}"));
    }

    let resp = req
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| whycode_core::Error::Llm(format!("models list HTTP: {e}")))?;

    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| whycode_core::Error::Llm(format!("models list body: {e}")))?;

    if !status.is_success() {
        let snippet: String = body.chars().take(200).collect();
        return Err(whycode_core::Error::Llm(format!(
            "models list {status}: {snippet}"
        )));
    }

    let json: Value = serde_json::from_str(&body)
        .map_err(|e| whycode_core::Error::Llm(format!("models list JSON: {e}")))?;

    Ok(parse_models_json(&json, &url))
}

/// Resolve base URL for listing models from provider config fields only.
pub fn base_url_from_provider_config(pc: &whycode_core::types::ProviderConfig) -> Option<String> {
    pc.base_url
        .as_ref()
        .or(pc.api_base.as_ref())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalize_models_url_variants() {
        assert_eq!(
            normalize_models_url("http://gateway.example/v1"),
            "http://gateway.example/v1/models"
        );
        assert_eq!(
            normalize_models_url("http://gateway.example/v1/chat/completions"),
            "http://gateway.example/v1/models"
        );
        assert_eq!(
            normalize_models_url("http://gateway.example/v1/models"),
            "http://gateway.example/v1/models"
        );
        assert_eq!(
            normalize_models_url("http://host:8080"),
            "http://host:8080/v1/models"
        );
    }

    #[test]
    fn catalog_request_requires_config_base_url() {
        use whycode_config::Config;
        use whycode_core::types::ProviderConfig;

        let mut cfg = Config::default();
        // No base → no fetch request (do not invent hosts).
        cfg.providers.insert(
            "naked".into(),
            ProviderConfig {
                name: "naked".into(),
                api_key: Some("sk-test".into()),
                api_base: None,
                base_url: None,
                headers: None,
                models: vec![],
                tool_arguments: None,
                extra: Default::default(),
            },
        );
        assert!(catalog_request_from_config(&cfg, "naked", Some("sk-test")).is_none());

        cfg.providers.insert(
            "gw".into(),
            ProviderConfig {
                name: "gw".into(),
                api_key: Some("sk-from-config".into()),
                api_base: None,
                base_url: Some("http://gateway.example/v1".into()),
                headers: Some(HashMap::from([("x-api-key".into(), "header-key".into())])),
                models: vec![],
                tool_arguments: None,
                extra: Default::default(),
            },
        );
        let req = catalog_request_from_config(&cfg, "gw", Some("sk-runtime")).unwrap();
        assert_eq!(req.base_url, "http://gateway.example/v1");
        // Config key wins over runtime.
        assert_eq!(req.api_key.as_deref(), Some("sk-from-config"));
        assert_eq!(
            req.headers.get("x-api-key").map(String::as_str),
            Some("header-key")
        );
    }

    #[test]
    fn parse_omniroute_style_models() {
        let json = json!({
            "object": "list",
            "data": [
                {
                    "id": "auto/best-coding",
                    "context_length": 1_050_000,
                    "max_input_tokens": 1_050_000,
                    "max_output_tokens": 1_048_576,
                },
                {
                    "id": "gpt-4o",
                    "context_length": 128_000,
                },
                {
                    "id": "no-meta",
                }
            ]
        });
        let cat = parse_models_json(&json, "http://example/v1/models");
        assert_eq!(cat.context_window("auto/best-coding"), Some(1_050_000));
        assert_eq!(cat.context_window("gpt-4o"), Some(128_000));
        assert_eq!(cat.context_window("no-meta"), None);
        assert_eq!(
            cat.max_output_tokens.get("auto/best-coding"),
            Some(&1_048_576)
        );
    }

    #[test]
    fn parse_vllm_max_model_len() {
        let json = json!({
            "data": [{ "id": "meta-llama", "max_model_len": 8192 }]
        });
        let cat = parse_models_json(&json, "u");
        assert_eq!(cat.context_window("meta-llama"), Some(8192));
    }

    #[test]
    fn context_window_from_nested_openrouter() {
        let m = json!({
            "id": "x",
            "top_provider": { "context_length": 200_000 }
        });
        assert_eq!(context_window_from_model_value(&m), Some(200_000));
    }

    #[test]
    fn context_window_for_model_id_exact_and_suffix() {
        let json = json!({
            "data": [
                { "id": "trk/moonshotai/kimi-k3-free", "context_length": 128_000 },
                { "id": "other", "context_length": 1_000 }
            ]
        });
        assert_eq!(
            context_window_for_model_id(&json, "trk/moonshotai/kimi-k3-free"),
            Some(128_000)
        );
        assert_eq!(context_window_for_model_id(&json, "missing"), None);
    }
}
