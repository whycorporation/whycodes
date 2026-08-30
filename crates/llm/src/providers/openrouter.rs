/// OpenRouter LLM provider.
/// OpenAI-compatible API at openrouter.ai with HTTP-Referer and X-Title headers.
use async_stream::stream;
use futures::stream::Stream;
use serde_json::Value;
use std::pin::Pin;
use whycodes_core::types::{LlmRequest, LlmResponse, StreamEvent};

use crate::provider::LlmProvider;
use async_trait::async_trait;

pub struct OpenRouterProvider {
    name: String,
    /// Optional site URL for the HTTP-Referer header
    pub site_url: Option<String>,
    /// Optional site name for the X-Title header
    pub site_name: Option<String>,
}

impl OpenRouterProvider {
    pub fn new() -> Self {
        Self {
            name: "openrouter".to_string(),
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

#[async_trait]
impl LlmProvider for OpenRouterProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_base_url(&self) -> &str {
        "https://openrouter.ai/api/v1/chat/completions"
    }

    async fn complete(
        &self,
        request: &LlmRequest,
        api_key: &str,
        model: &str,
    ) -> whycodes_core::Result<LlmResponse> {
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
            .map_err(|e| whycodes_core::Error::Llm(format!("HTTP error: {e}")))?;

        let status = resp.status();
        let json: Value = resp
            .json()
            .await
            .map_err(|e| whycodes_core::Error::Llm(format!("JSON parse error: {e}")))?;

        if !status.is_success() {
            let err_msg = json["error"]["message"].as_str().unwrap_or("Unknown error");
            return Err(whycodes_core::Error::Llm(format!(
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
    }

    async fn stream(
        &self,
        request: &LlmRequest,
        api_key: &str,
        model: &str,
    ) -> whycodes_core::Result<Pin<Box<dyn Stream<Item = whycodes_core::Result<StreamEvent>> + Send>>>
    {
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
            .map_err(|e| whycodes_core::Error::Llm(format!("HTTP error: {e}")))?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(whycodes_core::Error::Llm(format!(
                "OpenRouter API error: {}",
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
                        yield Err(crate::openai_compat::stream_chunk_error("openrouter", e));
                    }
                }
            }
        };

        Ok(Box::pin(s))
    }
}

impl Default for OpenRouterProvider {
    fn default() -> Self {
        Self::new()
    }
}
