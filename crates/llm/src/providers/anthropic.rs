/// Anthropic Claude LLM provider implementation.
/// Supports streaming with extended thinking via the Anthropic Messages API.
use async_stream::stream;
use futures::stream::Stream;
use serde_json::Value;
use std::pin::Pin;
use whycode_core::types::{
    ContentBlock, LlmRequest, LlmResponse, Message, StreamEvent, ToolDefinition, Usage,
};

use crate::provider::LlmProvider;
use async_trait::async_trait;

/// Usage on a `message_delta` event.
///
/// Anthropic's documented SSE shape is
/// `{"type":"message_delta","delta":{...},"usage":{"output_tokens":N}}`.
/// A few proxies nest `usage` inside `delta`. Either way, `output_tokens`
/// is a running snapshot (not a delta) — the agent folds with `max`.
fn usage_from_message_delta(event: &Value) -> Option<(u64, u64)> {
    let usage = event
        .get("usage")
        .filter(|v| v.is_object())
        .or_else(|| event.pointer("/delta/usage").filter(|v| v.is_object()))?;
    crate::usage_dump::dump_raw_usage("anthropic", usage);
    let input = usage
        .get("input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output = usage
        .get("output_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if input == 0 && output == 0 {
        None
    } else {
        Some((input, output))
    }
}

pub struct AnthropicProvider {
    name: String,
}

/// POST with the right auth header for the credential type.
///
/// OAuth subscription tokens (`sk-ant-oat…`, from `whycode auth login
/// anthropic`) must go in `Authorization: Bearer` with the oauth beta flag;
/// plain API keys go in `x-api-key`. Sending an OAuth token as `x-api-key`
/// is rejected by the API.
fn authed_post(url: &str, api_key: &str) -> reqwest::RequestBuilder {
    let req = crate::client_identity::post(url).header("anthropic-version", "2023-06-01");
    if api_key.starts_with("sk-ant-oat") {
        req.header("Authorization", format!("Bearer {api_key}"))
            .header("anthropic-beta", "oauth-2025-04-20")
    } else {
        req.header("x-api-key", api_key)
    }
}

impl AnthropicProvider {
    pub fn new() -> Self {
        Self {
            name: "anthropic".to_string(),
        }
    }

    pub fn build_body(&self, request: &LlmRequest, model: &str) -> Value {
        let mut body = serde_json::json!({
            "model": model,
            "max_tokens": request.max_tokens.unwrap_or(4096),
            "messages": self.convert_messages(&request.messages),
            "stream": true,
        });

        // System as plain string first; cache policy promotes + marks.
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

        if request.thinking.is_some() {
            body["thinking"] = serde_json::json!({"type": "enabled", "budget_tokens": 4000});
        }

        // OpenCode-parity: last tool + system + latest user message.
        if request.use_prompt_cache {
            crate::cache::apply_anthropic_cache_policy(
                &mut body,
                &crate::cache::CacheConfig::default(),
            );
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
        // cache_control is applied later by `apply_anthropic_cache_policy`.
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
        let mut body = self.build_body(request, model);
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
        crate::usage_dump::dump_raw_usage("anthropic", &usage);
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

        let resp = crate::oauth_refresh::send_with_refresh_retry(self.name(), &api_key, |key| {
            authed_post(self.default_base_url(), key).json(&body)
        })
        .await?;

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
                                            crate::usage_dump::dump_raw_usage("anthropic", usage);
                                            yield Ok(StreamEvent::Usage {
                                                input_tokens: usage["input_tokens"].as_u64().unwrap_or(0),
                                                output_tokens: 0,
                                            });
                                            // Cache tokens are billed separately from input_tokens
                                            // and only Anthropic reports them, so they travel as
                                            // their own event rather than as empty fields on every
                                            // other provider's usage.
                                            let created = usage["cache_creation_input_tokens"].as_u64().unwrap_or(0);
                                            let read = usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
                                            if created > 0 || read > 0 {
                                                yield Ok(StreamEvent::CacheUsage {
                                                    creation_input_tokens: created,
                                                    read_input_tokens: read,
                                                });
                                            }
                                        }
                                    }
                                    Some("message_delta") => {
                                        if let Some(delta) = event["delta"].as_object()
                                            && let Some(sr) = delta["stop_reason"].as_str()
                                        {
                                            yield Ok(StreamEvent::MessageDelta {
                                                delta: serde_json::json!({"stop_reason": sr}),
                                            });
                                        }
                                        // Official SSE puts `usage` as a sibling
                                        // of `delta`; some proxies nest it inside.
                                        if let Some((input, output)) =
                                            usage_from_message_delta(&event)
                                        {
                                            yield Ok(StreamEvent::Usage {
                                                input_tokens: input,
                                                output_tokens: output,
                                            });
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

#[cfg(test)]
mod tests {
    use super::usage_from_message_delta;
    use serde_json::json;

    #[test]
    fn usage_sibling_of_delta_is_official_shape() {
        let event = json!({
            "type": "message_delta",
            "delta": { "stop_reason": "end_turn" },
            "usage": { "output_tokens": 15 }
        });
        assert_eq!(usage_from_message_delta(&event), Some((0, 15)));
    }

    #[test]
    fn usage_nested_in_delta_is_accepted() {
        let event = json!({
            "type": "message_delta",
            "delta": { "stop_reason": "end_turn", "usage": { "output_tokens": 9 } }
        });
        assert_eq!(usage_from_message_delta(&event), Some((0, 9)));
    }

    #[test]
    fn sibling_usage_wins_over_empty_nested() {
        let event = json!({
            "type": "message_delta",
            "delta": { "stop_reason": "end_turn" },
            "usage": { "input_tokens": 40, "output_tokens": 12 }
        });
        assert_eq!(usage_from_message_delta(&event), Some((40, 12)));
    }

    #[test]
    fn missing_usage_is_none() {
        let event = json!({
            "type": "message_delta",
            "delta": { "stop_reason": "end_turn" }
        });
        assert!(usage_from_message_delta(&event).is_none());
    }
}
