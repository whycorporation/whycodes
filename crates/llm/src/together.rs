/// Together AI LLM provider.
/// OpenAI-compatible API at api.together.xyz.
use async_stream::stream;
use futures::stream::Stream;
use serde_json::Value;
use std::pin::Pin;
use whycode_core::types::{ContentBlock, LlmRequest, LlmResponse, StreamEvent, Usage};

use async_trait::async_trait;
use super::provider::LlmProvider;

pub struct TogetherProvider {
    name: String,
}

impl TogetherProvider {
    pub fn new() -> Self {
        Self {
            name: "together".to_string(),
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
        let mut messages: Vec<Value> = Vec::new();

        if !request.system.is_empty() {
            messages.push(serde_json::json!({
                "role": "system",
                "content": request.system
            }));
        }

        for msg in &request.messages {
            let role = match msg.role {
                whycode_core::types::Role::Assistant => "assistant",
                whycode_core::types::Role::User => "user",
                whycode_core::types::Role::System => "system",
                whycode_core::types::Role::Tool => "tool",
            };

            let content = match &msg.content {
                whycode_core::types::MessageContent::Text(text) => {
                    Value::String(text.clone())
                }
                whycode_core::types::MessageContent::Blocks(blocks) => {
                    let parts: Vec<Value> = blocks
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
                                    "type": "image_url",
                                    "image_url": {"url": format!("data:{};base64,{}", media_type, data)}
                                }),
                                whycode_core::types::ImageSource::Url { url } => {
                                    serde_json::json!({
                                        "type": "image_url",
                                        "image_url": {"url": url}
                                    })
                                }
                            },
                            ContentBlock::ToolUse { id, name, input } => serde_json::json!({
                                "type": "tool_use", "id": id, "name": name, "input": input
                            }),
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                ..
                            } => serde_json::json!({
                                "type": "tool_result",
                                "tool_use_id": tool_use_id,
                                "content": content
                            }),
                        })
                        .collect();
                    Value::Array(parts)
                }
            };

            let mut msg_obj = serde_json::json!({
                "role": role,
                "content": content
            });

            if let Some(tool_call_id) = &msg.tool_call_id {
                msg_obj["tool_call_id"] = Value::String(tool_call_id.clone());
            }
            if let Some(name) = &msg.name {
                msg_obj["name"] = Value::String(name.clone());
            }

            messages.push(msg_obj);
        }

        messages
    }

    fn convert_tools(&self, tools: &[whycode_core::types::ToolDefinition]) -> Vec<Value> {
        tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters
                    }
                })
            })
            .collect()
    }
}

#[async_trait]
impl LlmProvider for TogetherProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_base_url(&self) -> &str {
        "https://api.together.xyz/v1/chat/completions"
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
            .header("Authorization", format!("Bearer {}", api_key))
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
                "Together API error ({}): {}",
                status, err_msg
            )));
        }

        let choice = &json["choices"][0];
        let message = &choice["message"];

        let mut content: Vec<ContentBlock> = Vec::new();
        if let Some(text) = message["content"].as_str()
            && !text.is_empty() {
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
                    input: func["arguments"].clone(),
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
                cache_read_input_tokens: None,
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

        let client = reqwest::Client::new();
        let resp = client
            .post(self.default_base_url())
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| whycode_core::Error::Llm(format!("HTTP error: {e}")))?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(whycode_core::Error::Llm(format!(
                "Together API error: {}",
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
                                    let tc = &tool_calls[0];
                                    if let Some(id) = tc["id"].as_str() {
                                        yield Ok(StreamEvent::ToolUse {
                                            id: id.to_string(),
                                            name: tc["function"]["name"].as_str().unwrap_or("").to_string(),
                                            input: tc["function"]["arguments"].clone(),
                                        });
                                    } else if let Some(args) = tc["function"]["arguments"].as_str() {
                                        yield Ok(StreamEvent::ToolUseDelta {
                                            id: tc.get("index").map(|i| i.to_string()).unwrap_or_default(),
                                            input_json_delta: args.to_string(),
                                        });
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

impl Default for TogetherProvider {
    fn default() -> Self {
        Self::new()
    }
}
