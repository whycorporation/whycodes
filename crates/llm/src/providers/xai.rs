/// xAI Grok LLM provider.
///
/// Console API keys (`xai-…`) use `https://api.x.ai/v1/chat/completions`.
/// SuperGrok / X Premium tokens from `whycodes auth login xai` are rejected
/// there — they authorize the Grok Build chat proxy at
/// `cli-chat-proxy.grok.com`. Extra proxy headers (`X-XAI-Token-Auth`, …)
/// come from a loaded xAI auth plugin; core traffic is WhyCodes.
use async_stream::stream;
use serde_json::Value;
use whycodes_core::types::{LlmRequest, LlmResponse, StreamEvent};

use crate::provider::{
    LlmProvider, ProviderEventStream, ProviderResponseFuture, ProviderStreamFuture,
};

pub struct XaiProvider {
    name: String,
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

/// Chat-completions URL for this credential.
pub fn inference_url(api_key: &str) -> &'static str {
    if is_xai_oauth_token(api_key) {
        SUBSCRIPTION_CHAT_URL
    } else {
        CONSOLE_CHAT_URL
    }
}

fn authed_post(api_key: &str) -> reqwest::RequestBuilder {
    if is_xai_oauth_token(api_key) {
        crate::client_identity::post_for_provider(SUBSCRIPTION_CHAT_URL, "xai")
            .header("Authorization", format!("Bearer {api_key}"))
    } else {
        crate::client_identity::post(CONSOLE_CHAT_URL)
            .header("Authorization", format!("Bearer {api_key}"))
    }
}

impl XaiProvider {
    pub fn new() -> Self {
        Self {
            name: "xai".to_string(),
        }
    }

    pub fn build_body(&self, request: &LlmRequest, model: &str) -> Value {
        let mut body = serde_json::json!({
            "model": model,
            "messages": self.convert_messages(request),
            "stream": true,
        });

        if let Some(max_tokens) = request.max_tokens {
            body["max_tokens"] = max_tokens.into();
        }

        if !request.tools.is_empty() {
            body["tools"] = serde_json::Value::Array(self.convert_tools(&request.tools));
            body["tool_choice"] = serde_json::json!("auto");
            body["parallel_tool_calls"] = serde_json::json!(true);
        }

        crate::openai_compat::apply_sampling(&mut body, request);

        crate::thinking::ThinkingConfig::apply_openai_effort(&mut body, request.thinking.as_ref());

        body
    }

    fn convert_messages(&self, request: &LlmRequest) -> Vec<Value> {
        crate::openai_compat::convert_messages(request)
    }

    fn convert_tools(&self, tools: &[whycodes_core::types::ToolDefinition]) -> Vec<Value> {
        crate::openai_compat::convert_tools(tools)
    }
}

impl LlmProvider for XaiProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_base_url(&self) -> &str {
        CONSOLE_CHAT_URL
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
                authed_post(key).json(&body)
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
                authed_post(key).json(&body)
            })
            .await?;

            if !resp.status().is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(whycodes_core::Error::llm(format!(
                    "xAI API error: {}",
                    text
                )));
            }

            let s = stream! {
                let mut stream = resp.bytes_stream();
                let mut buffer = String::new();

                while let Some(chunk) = futures::StreamExt::next(&mut stream).await {
                    match chunk {
                        Ok(bytes) => {
                            buffer.push_str(&String::from_utf8_lossy(&bytes));
                            while let Some(pos) = buffer.find('\n') {
                                let line = buffer[..pos].trim().to_string();
                                buffer = buffer[pos + 1..].to_string();

                                if line.is_empty() || !line.starts_with("data: ") {
                                    continue;
                                }

                                let data = &line[6..];
                                if data == "[DONE]" {
                                    yield Ok(StreamEvent::MessageStop);
                                    return;
                                }

                                if let Ok(event) = serde_json::from_str::<Value>(data) {
                                    let choice = &event["choices"][0];
                                    let delta = &choice["delta"];

                                    for ev in crate::openai_compat::stream_events_for_chat_delta(delta) {
                                        yield Ok(ev);
                                    }

                                    if let Some(ev) =
                                        crate::openai_compat::stream_usage_from_chunk(&event)
                                    {
                                        yield Ok(ev);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            yield Err(crate::openai_compat::stream_chunk_error("xai", e));
                        }
                    }
                }
            };

            Ok(Box::pin(s) as ProviderEventStream)
        })
    }
}

impl Default for XaiProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_tokens_are_distinguished_from_console_keys() {
        assert!(is_xai_oauth_token("eyJhbGciOiJ.eyJzdWIiOiJx.sig"));
        assert!(is_xai_oauth_token("opaque-oauth-token"));
        assert!(!is_xai_oauth_token("xai-abc123"));
        assert!(!is_xai_oauth_token(""));
        assert_eq!(
            inference_url("eyJhbGciOiJ.eyJzdWIiOiJx.sig"),
            SUBSCRIPTION_CHAT_URL
        );
        assert_eq!(inference_url("xai-abc123"), CONSOLE_CHAT_URL);
    }
}
