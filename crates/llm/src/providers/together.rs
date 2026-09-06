/// Together AI LLM provider.
/// OpenAI-compatible API at api.together.xyz.
use serde_json::Value;
use whycodes_core::types::{LlmRequest, LlmResponse};

use crate::provider::{LlmProvider, ProviderResponseFuture, ProviderStreamFuture};

pub struct TogetherProvider {
    name: String,
    chat_url: String,
}

impl TogetherProvider {
    pub fn new() -> Self {
        Self::from_base(None)
    }

    pub fn from_base(base: Option<&str>) -> Self {
        Self {
            name: "together".to_string(),
            chat_url: match base.map(str::trim).filter(|s| !s.is_empty()) {
                Some(raw) => super::custom::normalize_chat_completions_url(raw),
                None => "https://api.together.xyz/v1/chat/completions".to_string(),
            },
        }
    }

    pub fn build_body(&self, request: &LlmRequest, model: &str) -> Value {
        crate::openai_compat::chat_completions_body(request, model, self.convert_messages(request))
    }

    fn convert_messages(&self, request: &LlmRequest) -> Vec<Value> {
        crate::openai_compat::convert_messages(request)
    }
}

impl LlmProvider for TogetherProvider {
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

            let resp = crate::client_identity::post(self.default_base_url())
                .header("Authorization", format!("Bearer {}", api_key))
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
                    "Together API error ({}): {}",
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

            let resp = crate::client_identity::post(self.default_base_url())
                .header("Authorization", format!("Bearer {}", api_key))
                .json(&body)
                .send()
                .await
                .map_err(|e| whycodes_core::Error::llm(format!("HTTP error: {e}")))?;

            if !resp.status().is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(whycodes_core::Error::llm(format!(
                    "Together API error: {}",
                    text
                )));
            }

            Ok(crate::openai_compat::chat_sse_stream(resp, "together"))
        })
    }
}

impl Default for TogetherProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "together_tests.rs"]
mod tests;
