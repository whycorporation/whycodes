/// OpenRouter LLM provider.
/// OpenAI-compatible API at openrouter.ai with HTTP-Referer and X-Title headers.
use serde_json::Value;
use whycodes_core::types::{LlmRequest, LlmResponse};

use crate::provider::{LlmProvider, ProviderResponseFuture, ProviderStreamFuture};

pub struct OpenRouterProvider {
    name: String,
    chat_url: String,
    /// Optional site URL for the HTTP-Referer header
    pub site_url: Option<String>,
    /// Optional site name for the X-Title header
    pub site_name: Option<String>,
}

impl OpenRouterProvider {
    pub fn new() -> Self {
        Self::from_base(None)
    }

    pub fn from_base(base: Option<&str>) -> Self {
        Self {
            name: "openrouter".to_string(),
            chat_url: match base.map(str::trim).filter(|s| !s.is_empty()) {
                Some(raw) => super::custom::normalize_chat_completions_url(raw),
                None => "https://openrouter.ai/api/v1/chat/completions".to_string(),
            },
            // Default to whycodes identity; override via `with_site`.
            site_url: Some(crate::client_identity::HTTP_REFERER.to_string()),
            site_name: Some(crate::client_identity::X_TITLE.to_string()),
        }
    }

    pub fn with_site(mut self, url: String, name: String) -> Self {
        self.site_url = Some(url);
        self.site_name = Some(name);
        self
    }

    pub fn build_body(&self, request: &LlmRequest, model: &str) -> Value {
        crate::openai_compat::chat_completions_body(request, model, self.convert_messages(request))
    }

    fn convert_messages(&self, request: &LlmRequest) -> Vec<Value> {
        crate::openai_compat::convert_messages(request)
    }
}

impl LlmProvider for OpenRouterProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_base_url(&self) -> &str {
        &self.chat_url
    }

    fn complete<'a>(
        &'a self,
        request: &'a LlmRequest,
        api_key: &'a str,
        model: &'a str,
    ) -> ProviderResponseFuture<'a> {
        Box::pin(async move {
            let mut body = self.build_body(request, model);
            body["stream"] = serde_json::Value::Bool(false);

            let mut req = crate::client_identity::post(self.default_base_url())
                .header("Authorization", format!("Bearer {}", api_key))
                .header("Content-Type", "application/json");

            // `with_site` overrides the default whycodes identity headers.
            if let Some(ref site_url) = self.site_url {
                req = req.header("HTTP-Referer", site_url);
            }
            if let Some(ref site_name) = self.site_name {
                req = req.header("X-Title", site_name);
            }

            let resp = req
                .json(&body)
                .send()
                .await
                .map_err(|e| whycodes_core::Error::llm(format!("HTTP error: {e}")))?;

            let status = resp.status();
            let json: Value = resp
                .json()
                .await
                .map_err(|e| whycodes_core::Error::llm(format!("JSON parse error: {e}")))?;

            if !status.is_success() {
                let err_msg = json["error"]["message"].as_str().unwrap_or("Unknown error");
                return Err(whycodes_core::Error::llm(format!(
                    "OpenRouter API error ({}): {}",
                    status, err_msg
                )));
            }

            let choice = &json["choices"][0];
            let message = &choice["message"];
            let content = crate::openai_compat::content_blocks_from_chat_message(message);

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
        api_key: &'a str,
        model: &'a str,
    ) -> ProviderStreamFuture<'a> {
        Box::pin(async move {
            let mut body = self.build_body(request, model);
            crate::openai_compat::attach_stream_usage_option(&mut body);

            let mut req = crate::client_identity::post(self.default_base_url())
                .header("Authorization", format!("Bearer {}", api_key))
                .header("Content-Type", "application/json");

            if let Some(ref site_url) = self.site_url {
                req = req.header("HTTP-Referer", site_url);
            }
            if let Some(ref site_name) = self.site_name {
                req = req.header("X-Title", site_name);
            }

            let resp = req
                .json(&body)
                .send()
                .await
                .map_err(|e| whycodes_core::Error::llm(format!("HTTP error: {e}")))?;

            if !resp.status().is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(whycodes_core::Error::llm(format!(
                    "OpenRouter API error: {}",
                    text
                )));
            }

            Ok(crate::openai_compat::chat_sse_stream(resp, "openrouter"))
        })
    }
}

impl Default for OpenRouterProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "openrouter_tests.rs"]
mod tests;
