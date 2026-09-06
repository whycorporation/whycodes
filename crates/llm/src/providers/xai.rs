/// xAI Grok LLM provider.
///
/// Console API keys (`xai-…`) use `https://api.x.ai/v1/chat/completions`.
/// SuperGrok / X Premium tokens from `whycodes auth login xai` are rejected
/// there — they authorize the Grok Build chat proxy at
/// `cli-chat-proxy.grok.com`. Extra proxy headers (`X-XAI-Token-Auth`, …)
/// come from a loaded xAI auth plugin; core traffic is WhyCodes.
use serde_json::Value;
use whycodes_core::types::{LlmRequest, LlmResponse};

use crate::provider::{LlmProvider, ProviderResponseFuture, ProviderStreamFuture};

pub struct XaiProvider {
    name: String,
    /// Optional override for tests (`from_base`). Production stays on the
    /// console or subscription URL chosen from the token.
    chat_url: Option<String>,
}

/// Console chat-completions endpoint (`XAI_API_KEY`).
pub const CONSOLE_CHAT_URL: &str = "https://api.x.ai/v1/chat/completions";
/// Grok Build subscription proxy. OAuth tokens from `auth.x.ai` only work here.
pub const SUBSCRIPTION_CHAT_URL: &str = "https://cli-chat-proxy.grok.com/v1/chat/completions";

/// True when `key` is a SuperGrok / X Premium OAuth token rather than a
/// console API key (`xai-…`). Access tokens may be JWTs or opaque.
pub fn is_xai_oauth_token(key: &str) -> bool {
    !key.is_empty() && !key.starts_with("xai-")
}

#[cfg(test)]
thread_local! {
    pub(crate) static TEST_SUBSCRIPTION_URL: std::cell::RefCell<Option<&'static str>> =
        const { std::cell::RefCell::new(None) };
}

fn subscription_chat_url() -> &'static str {
    #[cfg(test)]
    if let Some(url) = TEST_SUBSCRIPTION_URL.with(|c| *c.borrow()) {
        return url;
    }
    SUBSCRIPTION_CHAT_URL
}

/// Chat-completions URL for this credential.
pub fn inference_url(api_key: &str) -> &'static str {
    if is_xai_oauth_token(api_key) {
        subscription_chat_url()
    } else {
        CONSOLE_CHAT_URL
    }
}

fn authed_post(url: &str, api_key: &str) -> reqwest::RequestBuilder {
    if is_xai_oauth_token(api_key) && url == subscription_chat_url() {
        crate::client_identity::post_for_provider(url, "xai")
            .header("Authorization", format!("Bearer {api_key}"))
    } else {
        crate::client_identity::post(url).header("Authorization", format!("Bearer {api_key}"))
    }
}

impl XaiProvider {
    pub fn new() -> Self {
        Self::from_base(None)
    }

    pub fn from_base(base: Option<&str>) -> Self {
        Self {
            name: "xai".to_string(),
            chat_url: base
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(super::custom::normalize_chat_completions_url),
        }
    }

    fn request_url(&self, api_key: &str) -> String {
        self.chat_url
            .clone()
            .unwrap_or_else(|| inference_url(api_key).to_string())
    }

    pub fn build_body(&self, request: &LlmRequest, model: &str) -> Value {
        crate::openai_compat::chat_completions_body(request, model, self.convert_messages(request))
    }

    fn convert_messages(&self, request: &LlmRequest) -> Vec<Value> {
        crate::openai_compat::convert_messages(request)
    }
}

impl LlmProvider for XaiProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_base_url(&self) -> &str {
        self.chat_url.as_deref().unwrap_or(CONSOLE_CHAT_URL)
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

            let resp = crate::oauth_refresh::send_with_refresh_retry(self.name(), api_key, |key| {
                authed_post(&self.request_url(key), key).json(&body)
            })
            .await?;

            let status = resp.status();
            let json: Value = resp
                .json()
                .await
                .map_err(|e| whycodes_core::Error::llm(format!("JSON parse error: {e}")))?;

            if !status.is_success() {
                let err_msg = json["error"]["message"].as_str().unwrap_or("Unknown error");
                return Err(whycodes_core::Error::llm(format!(
                    "xAI API error ({}): {}",
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

            let resp = crate::oauth_refresh::send_with_refresh_retry(self.name(), api_key, |key| {
                authed_post(&self.request_url(key), key).json(&body)
            })
            .await?;

            if !resp.status().is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(whycodes_core::Error::llm(format!(
                    "xAI API error: {}",
                    text
                )));
            }

            Ok(crate::openai_compat::chat_sse_stream(resp, "xai"))
        })
    }
}

impl Default for XaiProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "xai_tests.rs"]
mod tests;
