/// xAI Grok LLM provider.
///
/// Console API keys (`xai-…`) use `https://api.x.ai/v1/chat/completions`.
/// SuperGrok / X Premium tokens from `whycode auth login xai` are rejected
/// there — they authorize the Grok Build chat proxy at
/// `cli-chat-proxy.grok.com` (same path the public Grok client uses), with
/// `X-XAI-Token-Auth: xai-grok-cli`.
use async_stream::stream;
use futures::stream::Stream;
use serde_json::Value;
use std::pin::Pin;
use whycode_core::types::{LlmRequest, LlmResponse, StreamEvent};

use crate::provider::LlmProvider;
use async_trait::async_trait;

pub struct XaiProvider {
    name: String,
}

/// Console chat-completions endpoint (`XAI_API_KEY`).
pub const CONSOLE_CHAT_URL: &str = "https://api.x.ai/v1/chat/completions";
/// Grok Build subscription proxy. OAuth tokens from `auth.x.ai` only work here.
pub const SUBSCRIPTION_CHAT_URL: &str = "https://cli-chat-proxy.grok.com/v1/chat/completions";

/// Product / version the public Grok CLI sends. The proxy gates auth
/// context on these (`x-grok-client-identifier` / `User-Agent`); a
/// whycode UA + GitHub Referer yields `upstream=Unauthenticated,
/// reason=no auth context` with an otherwise-valid token.
const GROK_CLIENT_IDENTIFIER: &str = "grok-shell";
const GROK_CLIENT_VERSION: &str = "1.0.5";

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

fn grok_user_agent() -> String {
    let arch = match std::env::consts::ARCH {
        "arm64" => "aarch64",
        other => other,
    };
    format!(
        "{GROK_CLIENT_IDENTIFIER}/{GROK_CLIENT_VERSION} ({}; {arch})",
        std::env::consts::OS
    )
}

fn authed_post(api_key: &str) -> reqwest::RequestBuilder {
    if is_xai_oauth_token(api_key) {
        // Do not use `client_identity::post`: HTTP-Referer / X-Title /
        // whycode User-Agent prevent the proxy from attaching a user.
        crate::client_identity::http_client()
            .post(SUBSCRIPTION_CHAT_URL)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("User-Agent", grok_user_agent())
            .header("X-XAI-Token-Auth", "xai-grok-cli")
            .header("x-authenticateresponse", "authenticate-response")
            .header("x-grok-client-mode", "interactive")
            .header("x-grok-client-identifier", GROK_CLIENT_IDENTIFIER)
            .header("x-grok-client-version", GROK_CLIENT_VERSION)
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

        if let Some(temp) = request.temperature {
            body["temperature"] = Value::Number(serde_json::Number::from_f64(temp as f64).unwrap());
        }

        if let Some(top_p) = request.top_p {
            body["top_p"] = Value::Number(serde_json::Number::from_f64(top_p as f64).unwrap());
        }

        crate::thinking::ThinkingConfig::apply_openai_effort(&mut body, request.thinking.as_ref());

        body
    }

    fn convert_messages(&self, request: &LlmRequest) -> Vec<Value> {
        crate::openai_compat::convert_messages(request)
    }

    fn convert_tools(&self, tools: &[whycode_core::types::ToolDefinition]) -> Vec<Value> {
        crate::openai_compat::convert_tools(tools)
    }
}

#[async_trait]
impl LlmProvider for XaiProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_base_url(&self) -> &str {
        CONSOLE_CHAT_URL
    }

    async fn complete(
        &self,
        request: &LlmRequest,
        api_key: &str,
        model: &str,
    ) -> whycode_core::Result<LlmResponse> {
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
            .map_err(|e| whycode_core::Error::Llm(format!("JSON parse error: {e}")))?;

        if !status.is_success() {
            let err_msg = json["error"]["message"].as_str().unwrap_or("Unknown error");
            return Err(whycode_core::Error::Llm(format!(
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
    }

    async fn stream(
        &self,
        request: &LlmRequest,
        api_key: &str,
        model: &str,
    ) -> whycode_core::Result<Pin<Box<dyn Stream<Item = whycode_core::Result<StreamEvent>> + Send>>>
    {
        let mut body = self.build_body(request, model);
        crate::openai_compat::attach_stream_usage_option(&mut body);

        let resp = crate::oauth_refresh::send_with_refresh_retry(self.name(), api_key, |key| {
            authed_post(key).json(&body)
        })
        .await?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(whycode_core::Error::Llm(format!("xAI API error: {}", text)));
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
                        yield Err(whycode_core::Error::Llm(format!("Stream error: {e}")));
                    }
                }
            }
        };

        Ok(Box::pin(s))
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
        let ua = grok_user_agent();
        assert!(ua.starts_with("grok-shell/"), "{ua}");
        assert!(ua.contains('(') && ua.contains(')'), "{ua}");
    }
}
