/// GitHub Copilot LLM provider.
///
/// The Copilot API is OpenAI-compatible (`api.githubcopilot.com`), so this
/// reuses the shared `openai_compat` helpers; the differences are the base
/// URL and the editor-identity headers Copilot expects. The credential is
/// the short-lived Copilot API token obtained via `whycodes auth login
/// github-copilot` (device flow → token exchange); refresh is handled by
/// `whycodes-auth` before the token reaches this provider.
use async_stream::stream;
use serde_json::Value;
use whycodes_core::types::{LlmRequest, LlmResponse, StreamEvent};

use crate::provider::{
    LlmProvider, ProviderEventStream, ProviderResponseFuture, ProviderStreamFuture,
};

pub struct CopilotProvider {
    name: String,
    chat_url: String,
}

/// POST with bearer auth. Extra editor-identity headers come from a loaded
/// Copilot auth plugin (`inference.headers`); core traffic is WhyCodes.
fn authed_post(url: &str, api_key: &str) -> reqwest::RequestBuilder {
    crate::client_identity::post_for_provider(url, "github-copilot")
        .header("Authorization", format!("Bearer {api_key}"))
}

impl CopilotProvider {
    pub fn new() -> Self {
        Self::from_base(None)
    }

    pub fn from_base(base: Option<&str>) -> Self {
        Self {
            name: "github-copilot".to_string(),
            chat_url: match base.map(str::trim).filter(|s| !s.is_empty()) {
                Some(raw) => super::custom::normalize_chat_completions_url(raw),
                None => "https://api.githubcopilot.com/chat/completions".to_string(),
            },
        }
    }
}

impl LlmProvider for CopilotProvider {
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
            // Same request shape as OpenAI chat completions.
            let mut body = super::openai::OpenAiProvider::new().build_body(request, model);
            body["stream"] = serde_json::Value::Bool(false);

            let resp = crate::oauth_refresh::send_with_refresh_retry(self.name(), api_key, |key| {
                authed_post(self.default_base_url(), key).json(&body)
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
                    "Copilot API error ({}): {}",
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
            let mut body = super::openai::OpenAiProvider::new().build_body(request, model);
            crate::openai_compat::attach_stream_usage_option(&mut body);

            let resp = crate::oauth_refresh::send_with_refresh_retry(self.name(), api_key, |key| {
                authed_post(self.default_base_url(), key).json(&body)
            })
            .await?;

            if !resp.status().is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(whycodes_core::Error::llm(format!(
                    "Copilot API error: {}",
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

                                    // Final include_usage chunk often has empty choices —
                                    // do not require finish_reason.
                                    if let Some(ev) =
                                        crate::openai_compat::stream_usage_from_chunk(&event)
                                    {
                                        yield Ok(ev);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            yield Err(crate::openai_compat::stream_chunk_error("github-copilot", e));
                        }
                    }
                }
            };

            Ok(Box::pin(s) as ProviderEventStream)
        })
    }
}

impl Default for CopilotProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::LlmProvider;

    #[test]
    fn from_base_blank_keeps_cloud_and_override_normalizes() {
        let cloud = CopilotProvider::from_base(Some("   "));
        assert!(cloud.default_base_url().contains("githubcopilot.com"));
        let local = CopilotProvider::from_base(Some("http://127.0.0.1:9/v1"));
        assert!(
            local.default_base_url().ends_with("/chat/completions"),
            "{}",
            local.default_base_url()
        );
        assert_eq!(CopilotProvider::default().name(), "github-copilot");
    }
}
