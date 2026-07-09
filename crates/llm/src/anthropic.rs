/// Anthropic Claude LLM provider implementation.
/// Supports streaming with extended thinking via the Anthropic Messages API.
use async_stream::stream;
use futures::stream::Stream;
use serde_json::Value;
use std::pin::Pin;
use whycode_core::types::{ContentBlock, LlmRequest, LlmResponse, Message, StreamEvent, ToolDefinition, Usage};

use async_trait::async_trait;
use super::provider::LlmProvider;

pub struct AnthropicProvider {
    name: String,
}

impl AnthropicProvider {
    pub fn new() -> Self {
        Self {
            name: "anthropic".to_string(),
        }
    }

    fn build_body(&self, request: &LlmRequest, model: &str) -> Value {
        let mut body = serde_json::json!({
            "model": model,
            "max_tokens": request.max_tokens.unwrap_or(4096),
            "messages": self.convert_messages(&request.messages),
            "stream": true,
        });

        if !request.system.is_empty() {
            body["system"] = Value::String(request.system.clone());
        }

        if !request.tools.is_empty() {
            body["tools"] = serde_json::Value::Array(self.convert_tools(&request.tools));
        }

        if let Some(temp) = request.temperature {
            body["temperature"] = Value::Number(serde_json::Number::from_f64(temp as f64).unwrap());
        }

        if let Some(top_p) = request.top_p {
            body["top_p"] = Value::Number(serde_json::Number::from_f64(top_p as f64).unwrap());
        }

        if request.thinking.unwrap_or(false) {
            body["thinking"] = serde_json::json!({"type": "enabled", "budget_tokens": 4000});
        }

        body
    }

    fn convert_messages(&self, messages: &[Message]) -> Vec<Value> {
        messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    whycode_core::types::Role::Assistant => "assistant",
                    whycode_core::types::Role::User => "user",
                    whycode_core::types::Role::System => "user", // system goes in top-level
                    whycode_core::types::Role::Tool => "user",
                };

                let content = match &m.content {
                    whycode_core::types::MessageContent::Text(text) => {
                        vec![serde_json::json!({"type": "text", "text": text})]
                    }
                    whycode_core::types::MessageContent::Blocks(blocks) => blocks
                        .iter()
                        .map(|b| match b {
                            ContentBlock::Text { text } => {
                                serde_json::json!({"type": "text", "text": text})
                            }
                            ContentBlock::Image { source } => match source {
                                whycode_core::types::ImageSource::Base64 {
                                    media_type,
                                    data,
                                } => serde_json::json!({
                                    "type": "image",
                                    "source": {"type": "base64", "media_type": media_type, "data": data}
                                }),
                                _ => serde_json::json!({"type": "text", "text": "[image]"}),
                            },
                            ContentBlock::ToolUse { id, name, input } => serde_json::json!({
                                "type": "tool_use", "id": id, "name": name, "input": input
                            }),
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                is_error,
                            } => serde_json::json!({
                                "type": "tool_result",
                                "tool_use_id": tool_use_id,
                                "content": content,
                                "is_error": is_error.unwrap_or(false)
                            }),
                        })
                        .collect(),
                };

                serde_json::json!({"role": role, "content": content})
            })
            .collect()
    }

    fn convert_tools(&self, tools: &[ToolDefinition]) -> Vec<Value> {
        tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters
                })
            })
            .collect()
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_base_url(&self) -> &str {
        "https://api.anthropic.com/v1/messages"
    }

    async fn complete(
        &self,
        request: &LlmRequest,
        api_key: &str,
        model: &str,
    ) -> whycode_core::Result<LlmResponse> {
        let client = reqwest::Client::new();
        let mut body = self.build_body(request, model);
        body["stream"] = serde_json::Value::Bool(false);

        let resp = client
            .post(self.default_base_url())
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
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
            let err_msg = json["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error");
            return Err(whycode_core::Error::Llm(format!(
                "Anthropic API error ({}): {}",
                status, err_msg
            )));
        }

        let content = json["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .map(|b| {
                        let btype = b["type"].as_str().unwrap_or("text");
                        match btype {
                            "text" => ContentBlock::Text {
                                text: b["text"].as_str().unwrap_or("").to_string(),
                            },
                            "tool_use" => ContentBlock::ToolUse {
                                id: b["id"].as_str().unwrap_or("").to_string(),
                                name: b["name"].as_str().unwrap_or("").to_string(),
                                input: b["input"].clone(),
                            },
                            _ => ContentBlock::Text {
                                text: "[unknown block]".to_string(),
                            },
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        let usage = json["usage"].clone();
        Ok(LlmResponse {
            content,
            stop_reason: json["stop_reason"].as_str().map(|s| s.to_string()),
            usage: Usage {
                input_tokens: usage["input_tokens"].as_u64().unwrap_or(0),
                output_tokens: usage["output_tokens"].as_u64().unwrap_or(0),
                cache_creation_input_tokens: usage["cache_creation_input_tokens"].as_u64(),
                cache_read_input_tokens: usage["cache_read_input_tokens"].as_u64(),
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
        let api_key = api_key.to_string();

        let client = reqwest::Client::new();
        let resp = client
            .post(self.default_base_url())
            .header("x-api-key", &api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .map_err(|e| whycode_core::Error::Llm(format!("HTTP error: {e}")))?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(whycode_core::Error::Llm(format!(
                "Anthropic API error: {}",
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
                                match event["type"].as_str() {
                                    Some("message_start") => {
                                        if let Some(msg) = event["message"].as_object() {
                                            let usage = &msg["usage"];
                                            yield Ok(StreamEvent::Usage {
                                                input_tokens: usage["input_tokens"].as_u64().unwrap_or(0),
                                                output_tokens: 0,
                                            });
                                        }
                                    }
                                    Some("message_delta") => {
                                        if let Some(delta) = event["delta"].as_object() {
                                            if let Some(sr) = delta["stop_reason"].as_str() {
                                                yield Ok(StreamEvent::MessageDelta {
                                                    delta: serde_json::json!({"stop_reason": sr}),
                                                });
                                            }
                                            let usage = &delta["usage"];
                                            if usage["output_tokens"].as_u64().unwrap_or(0) > 0 {
                                                yield Ok(StreamEvent::Usage {
                                                    input_tokens: 0,
                                                    output_tokens: usage["output_tokens"].as_u64().unwrap_or(0),
                                                });
                                            }
                                        }
                                    }
                                    Some("content_block_start") => {
                                        let block = &event["content_block"];
                                        match block["type"].as_str() {
                                            Some("tool_use") => {
                                                yield Ok(StreamEvent::ToolUse {
                                                    id: block["id"].as_str().unwrap_or("").to_string(),
                                                    name: block["name"].as_str().unwrap_or("").to_string(),
                                                    input: block["input"].clone(),
                                                });
                                            }
                                            Some("thinking") => {
                                                if let Some(thinking) = block["thinking"].as_str() {
                                                    yield Ok(StreamEvent::Thinking {
                                                        text: thinking.to_string(),
                                                    });
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                    Some("content_block_delta") => {
                                        let delta = &event["delta"];
                                        match delta["type"].as_str() {
                                            Some("text_delta") => {
                                                if let Some(text) = delta["text"].as_str() {
                                                    yield Ok(StreamEvent::TextDelta {
                                                        text: text.to_string(),
                                                    });
                                                }
                                            }
                                            Some("input_json_delta") => {
                                                if let Some(json) = delta["partial_json"].as_str() {
                                                    yield Ok(StreamEvent::ToolUseDelta {
                                                        id: String::new(),
                                                        input_json_delta: json.to_string(),
                                                    });
                                                }
                                            }
                                            Some("thinking_delta") => {
                                                if let Some(thinking) = delta["thinking"].as_str() {
                                                    yield Ok(StreamEvent::ThinkingDelta {
                                                        text: thinking.to_string(),
                                                    });
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                    Some("message_stop") => {
                                        yield Ok(StreamEvent::MessageStop);
                                    }
                                    Some("error") => {
                                        yield Err(whycode_core::Error::Llm(
                                            event["error"]["message"]
                                                .as_str()
                                                .unwrap_or("Unknown error")
                                                .to_string(),
                                        ));
                                    }
                                    _ => {}
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

impl Default for AnthropicProvider {
    fn default() -> Self {
        Self::new()
    }
}
