/// OpenRouter LLM provider.
/// OpenAI-compatible API at openrouter.ai with HTTP-Referer and X-Title headers.
use async_stream::stream;
use futures::stream::Stream;
use serde_json::Value;
use std::pin::Pin;
use whycode_core::types::{ContentBlock, LlmRequest, LlmResponse, StreamEvent, Usage};

use super::provider::LlmProvider;
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
            // Default to whycode identity; override via `with_site`.
            site_url: Some(super::client_identity::HTTP_REFERER.to_string()),
            site_name: Some(super::client_identity::X_TITLE.to_string()),
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
        }

        if let Some(temp) = request.temperature {
            body["temperature"] = Value::Number(serde_json::Number::from_f64(temp as f64).unwrap());
        }

        if let Some(top_p) = request.top_p {
            body["top_p"] = Value::Number(serde_json::Number::from_f64(top_p as f64).unwrap());
        }

        body
    }

    fn convert_messages(&self, request: &LlmRequest) -> Vec<Value> {
        super::openai_compat::convert_messages(request)
    }

    fn convert_tools(&self, tools: &[whycode_core::types::ToolDefinition]) -> Vec<Value> {
        super::openai_compat::convert_tools(tools)
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
    ) -> whycode_core::Result<LlmResponse> {
        let mut body = self.build_body(request, model);
        body["stream"] = serde_json::Value::Bool(false);

        let mut req = super::client_identity::post(self.default_base_url())
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json");

        // `with_site` overrides the default whycode identity headers.
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
            .map_err(|e| whycode_core::Error::Llm(format!("HTTP error: {e}")))?;

        let status = resp.status();
        let json: Value = resp
            .json()
            .await
            .map_err(|e| whycode_core::Error::Llm(format!("JSON parse error: {e}")))?;

        if !status.is_success() {
            let err_msg = json["error"]["message"].as_str().unwrap_or("Unknown error");
            return Err(whycode_core::Error::Llm(format!(
                "OpenRouter API error ({}): {}",
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
                    input: super::openai_compat::parse_tool_arguments(&func["arguments"]),
                });
            }
        }

        let usage = &json["usage"];
        Ok(LlmResponse {
            content,
            stop_reason: choice["finish_reason"].as_str().map(|s| s.to_string()),
            usage: Usage {
                input_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0),
                output_tokens: usage["completion_tokens"].as_u64().unwrap_or(0),
                cache_read_input_tokens: usage["prompt_tokens_details"]["cached_tokens"].as_u64(),
                cache_creation_input_tokens: None,
            },
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
        let body = self.build_body(request, model);

        let mut req = super::client_identity::post(self.default_base_url())
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
            .map_err(|e| whycode_core::Error::Llm(format!("HTTP error: {e}")))?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(whycode_core::Error::Llm(format!(
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

                                if let Some(text) = delta["content"].as_str()
                                    && !text.is_empty() {
                                        yield Ok(StreamEvent::TextDelta {
                                            text: text.to_string(),
                                        });
                                    }

                                if let Some(tool_calls) = delta["tool_calls"].as_array() {
                                    for ev in super::openai_compat::stream_events_for_tool_calls(tool_calls) {
                                        yield Ok(ev);
                                    }
                                }

                                if let Some(finish) = choice["finish_reason"].as_str()
                                    && !finish.is_empty()
                                        && let Some(usage) = event.get("usage") {
                                            yield Ok(StreamEvent::Usage {
                                                input_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0),
                                                output_tokens: usage["completion_tokens"].as_u64().unwrap_or(0),
                                            });
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

impl Default for OpenRouterProvider {
    fn default() -> Self {
        Self::new()
    }
}
