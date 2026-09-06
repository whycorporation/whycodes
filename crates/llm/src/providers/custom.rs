/// A generic OpenAI-compatible provider that can be configured at runtime.
/// Supports custom base URLs, headers, and authentication schemas.
use serde_json::Value;
use std::collections::HashMap;
use whycodes_core::types::{LlmRequest, LlmResponse, ToolArgumentsFormat};

use crate::provider::{LlmProvider, ProviderResponseFuture, ProviderStreamFuture};

/// A provider that works with any OpenAI-compatible API endpoint.
///
/// Configure via config.toml:
/// ```toml
/// [providers.my-custom]
/// name = "my-custom"
/// api_key = "sk-xxx"
/// base_url = "https://api.example.com/v1/chat/completions"
/// headers = { "X-Custom-Header" = "value" }
/// # Only if the gateway requires bare JSON objects for tool args:
/// # tool_arguments = "object"
/// ```
pub struct CustomProvider {
    name: String,
    base_url: String,
    api_key: Option<String>,
    headers: HashMap<String, String>,
    tool_arguments: ToolArgumentsFormat,
}

impl CustomProvider {
    /// Create a new custom provider with full configuration
    pub fn new(
        name: impl Into<String>,
        base_url: impl Into<String>,
        api_key: Option<String>,
        headers: HashMap<String, String>,
    ) -> Self {
        Self {
            name: name.into(),
            base_url: base_url.into(),
            api_key,
            headers,
            tool_arguments: ToolArgumentsFormat::JsonString,
        }
    }

    /// Create from config
    pub fn from_config(config: &whycodes_core::types::ProviderConfig) -> Self {
        // Accept either a bare `/v1` base or a full chat-completions URL.
        let url = normalize_chat_completions_url(
            config
                .base_url
                .as_deref()
                .or(config.api_base.as_deref())
                .unwrap_or("https://api.openai.com/v1"),
        );

        let mut headers = config.headers.clone().unwrap_or_default();
        // Add auth header if not already present
        if !headers.contains_key("Authorization")
            && let Some(key) = &config.api_key
            && !key.is_empty()
        {
            headers.insert("Authorization".to_string(), format!("Bearer {key}"));
        }

        Self {
            name: config.name.clone(),
            base_url: url,
            api_key: config.api_key.clone(),
            headers,
            tool_arguments: config.tool_arguments_format(),
        }
    }

    fn build_body(&self, request: &LlmRequest, model: &str) -> Value {
        crate::openai_compat::chat_completions_body(request, model, self.convert_messages(request))
    }

    fn convert_messages(&self, request: &LlmRequest) -> Vec<Value> {
        crate::openai_compat::convert_messages_with_format(request, self.tool_arguments)
    }

    fn build_request(&self, body: &Value) -> reqwest::RequestBuilder {
        // Identity first; config `headers` may override (e.g. custom User-Agent).
        let mut req = crate::client_identity::post(&self.base_url).json(body);

        for (key, value) in &self.headers {
            req = req.header(key, value);
        }

        // Fallback auth if no Authorization header set
        if !self.headers.contains_key("Authorization")
            && let Some(key) = &self.api_key
            && !key.is_empty()
        {
            req = req.header("Authorization", format!("Bearer {key}"));
        }

        req
    }
}

/// Ensure `base` points at the OpenAI chat-completions endpoint.
///
/// Configs usually store `http://host:port/v1`; the HTTP client posts to
/// `{base}/chat/completions`. If the path already ends with that suffix, leave
/// it alone.
pub fn normalize_chat_completions_url(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    if base.ends_with("/chat/completions") {
        base.to_string()
    } else {
        format!("{base}/chat/completions")
    }
}

impl LlmProvider for CustomProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_base_url(&self) -> &str {
        &self.base_url
    }

    fn complete<'a>(
        &'a self,
        request: &'a LlmRequest,
        _api_key: &'a str,
        model: &'a str,
    ) -> ProviderResponseFuture<'a> {
        Box::pin(async move {
            let mut body = self.build_body(request, model);
            body["stream"] = Value::Bool(false);

            let resp = self
                .build_request(&body)
                .send()
                .await
                .map_err(|e| whycodes_core::Error::llm(format!("HTTP error: {e}")))?;

            let status = resp.status();
            let json: Value = resp
                .json()
                .await
                .map_err(|e| whycodes_core::Error::llm(format!("JSON: {e}")))?;

            if !status.is_success() {
                let msg = json["error"]["message"].as_str().unwrap_or("unknown");
                return Err(whycodes_core::Error::llm(format!(
                    "{} API error ({}): {}",
                    self.name, status, msg
                )));
            }

            let choice = &json["choices"][0];
            let msg = &choice["message"];
            let content = crate::openai_compat::content_blocks_from_chat_message(msg);

            let usage = &json["usage"];
            Ok(LlmResponse {
                content,
                stop_reason: choice["finish_reason"].as_str().map(|s| s.to_string()),
                usage: crate::openai_compat::usage_from_chat_completion(usage),
                model: model.to_string(),
            })
        })
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
        _api_key: &'a str,
        model: &'a str,
    ) -> ProviderStreamFuture<'a> {
        Box::pin(async move {
            let mut body = self.build_body(request, model);
            crate::openai_compat::attach_stream_usage_option(&mut body);
            let resp = self
                .build_request(&body)
                .send()
                .await
                .map_err(|e| whycodes_core::Error::llm(format!("HTTP: {e}")))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                // Include (NNN) so retry_with_backoff / is_retryable can see 5xx.
                return Err(whycodes_core::Error::llm(format!(
                    "{} API error ({}): {}",
                    self.name,
                    status.as_u16(),
                    text
                )));
            }

            Ok(crate::openai_compat::chat_sse_stream(resp, &self.name))
        })
    }
}

/// Test for custom provider with auth modes
#[cfg(test)]
#[path = "custom_tests.rs"]
mod tests;
