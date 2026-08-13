/// GitHub Copilot LLM provider.
///
/// The Copilot API is OpenAI-compatible (`api.githubcopilot.com`), so this
/// reuses the shared `openai_compat` helpers; the differences are the base
/// URL and the editor-identity headers Copilot expects. The credential is
/// the short-lived Copilot API token obtained via `whycode auth login
/// github-copilot` (device flow → token exchange); refresh is handled by
/// `whycode-auth` before the token reaches this provider.
use async_stream::stream;
use futures::stream::Stream;
use serde_json::Value;
use std::pin::Pin;
use whycode_core::types::{ContentBlock, LlmRequest, LlmResponse, StreamEvent};

use crate::provider::LlmProvider;
use async_trait::async_trait;

pub struct CopilotProvider {
    name: String,
}

/// POST with the headers the Copilot API expects: bearer token plus the
/// editor identity pair it uses for client gating.
fn authed_post(url: &str, api_key: &str) -> reqwest::RequestBuilder {
    crate::client_identity::post(url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Editor-Version", "vscode/1.95.0")
        .header("Copilot-Integration-Id", "vscode-chat")
}

impl CopilotProvider {
    pub fn new() -> Self {
        Self {
            name: "github-copilot".to_string(),
        }
    }
}

#[async_trait]
impl LlmProvider for CopilotProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_base_url(&self) -> &str {
        "https://api.githubcopilot.com/chat/completions"
    }

    async fn complete(
        &self,
        request: &LlmRequest,
        api_key: &str,
        model: &str,
    ) -> whycode_core::Result<LlmResponse> {
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
            .map_err(|e| whycode_core::Error::Llm(format!("JSON parse error: {e}")))?;

        if !status.is_success() {
            let err_msg = json["error"]["message"].as_str().unwrap_or("Unknown error");
            return Err(whycode_core::Error::Llm(format!(
                "Copilot API error ({}): {}",
                status, err_msg
            )));
        }

        let choice = &json["choices"][0];
        let message = &choice["message"];

        let mut content: Vec<ContentBlock> = Vec::new();
        if let Some(text) = message["content"].as_str()
            && !text.is_empty()
        {
            content.push(ContentBlock::Text {
                text: text.to_string(),
            });
        }
        if let Some(tool_calls) = message["tool_calls"].as_array() {
            for tc in tool_calls {
                let func = &tc["function"];
                content.push(ContentBlock::ToolUse {
                    id: tc["id"].as_str().unwrap_or("").to_string(),
                    name: func["name"].as_str().unwrap_or("").to_string(),
                    input: crate::openai_compat::parse_tool_arguments(&func["arguments"]),
                });
            }
        }

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
        let mut body = super::openai::OpenAiProvider::new().build_body(request, model);
        crate::openai_compat::attach_stream_usage_option(&mut body);

        let resp = crate::oauth_refresh::send_with_refresh_retry(self.name(), api_key, |key| {
            authed_post(self.default_base_url(), key).json(&body)
        })
        .await?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(whycode_core::Error::Llm(format!(
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
                        yield Err(whycode_core::Error::Llm(format!("Stream error: {e}")));
                    }
                }
            }
        };

        Ok(Box::pin(s))
    }
}

impl Default for CopilotProvider {
    fn default() -> Self {
        Self::new()
    }
}
