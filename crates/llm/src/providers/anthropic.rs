/// Anthropic Claude LLM provider implementation.
/// Supports streaming with extended thinking via the Anthropic Messages API.
use async_stream::stream;
use serde_json::Value;
use whycodes_core::types::{
    ContentBlock, LlmRequest, LlmResponse, Message, StreamEvent, ToolDefinition, Usage,
};

use crate::provider::{
    LlmProvider, ProviderEventStream, ProviderResponseFuture, ProviderStreamFuture,
};

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

/// Map one SSE `data:` payload to zero or more stream events.
///
/// Extracted from `stream()` so wire-format handling stays unit-testable
/// without a live HTTP response (same seam as codex `events_for_payload`).
fn events_for_data(data: &str) -> Vec<whycodes_core::Result<StreamEvent>> {
    let Ok(event) = serde_json::from_str::<Value>(data) else {
        return Vec::new();
    };
    let mut out: Vec<whycodes_core::Result<StreamEvent>> = Vec::new();
    match event["type"].as_str() {
        Some("message_start") => {
            if let Some(msg) = event["message"].as_object() {
                let usage = &msg["usage"];
                crate::usage_dump::dump_raw_usage("anthropic", usage);
                out.push(Ok(StreamEvent::Usage {
                    input_tokens: usage["input_tokens"].as_u64().unwrap_or(0),
                    output_tokens: 0,
                }));
                // Cache tokens are billed separately from input_tokens
                // and only Anthropic reports them, so they travel as
                // their own event rather than as empty fields on every
                // other provider's usage.
                let created = usage["cache_creation_input_tokens"].as_u64().unwrap_or(0);
                let read = usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
                if created > 0 || read > 0 {
                    out.push(Ok(StreamEvent::CacheUsage {
                        creation_input_tokens: created,
                        read_input_tokens: read,
                    }));
                }
            }
        }
        Some("message_delta") => {
            if let Some(delta) = event["delta"].as_object()
                && let Some(sr) = delta["stop_reason"].as_str()
            {
                out.push(Ok(StreamEvent::MessageDelta {
                    delta: serde_json::json!({"stop_reason": sr}),
                }));
            }
            // Official SSE puts `usage` as a sibling
            // of `delta`; some proxies nest it inside.
            if let Some((input, output)) = usage_from_message_delta(&event) {
                out.push(Ok(StreamEvent::Usage {
                    input_tokens: input,
                    output_tokens: output,
                }));
            }
        }
        Some("content_block_start") => {
            let block = &event["content_block"];
            match block["type"].as_str() {
                Some("tool_use") => {
                    out.push(Ok(StreamEvent::ToolUse {
                        id: block["id"].as_str().unwrap_or("").to_string(),
                        name: block["name"].as_str().unwrap_or("").to_string(),
                        input: block["input"].clone(),
                    }));
                }
                Some("thinking") => {
                    if let Some(thinking) = block["thinking"].as_str()
                        && !thinking.is_empty()
                    {
                        out.push(Ok(StreamEvent::Thinking {
                            text: thinking.to_string(),
                        }));
                    }
                    if let Some(sig) = block["signature"].as_str()
                        && !sig.is_empty()
                    {
                        out.push(Ok(StreamEvent::ThinkingSignature {
                            signature: sig.to_string(),
                        }));
                    }
                }
                Some("redacted_thinking") => {
                    if let Some(data) = block["data"].as_str() {
                        out.push(Ok(StreamEvent::RedactedThinking {
                            data: data.to_string(),
                        }));
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
                        out.push(Ok(StreamEvent::TextDelta {
                            text: text.to_string(),
                        }));
                    }
                }
                Some("input_json_delta") => {
                    if let Some(json) = delta["partial_json"].as_str() {
                        out.push(Ok(StreamEvent::ToolUseDelta {
                            id: String::new(),
                            input_json_delta: json.to_string(),
                        }));
                    }
                }
                Some("thinking_delta") => {
                    if let Some(thinking) = delta["thinking"].as_str() {
                        out.push(Ok(StreamEvent::ThinkingDelta {
                            text: thinking.to_string(),
                        }));
                    }
                }
                Some("signature_delta") => {
                    if let Some(sig) = delta["signature"].as_str()
                        && !sig.is_empty()
                    {
                        out.push(Ok(StreamEvent::ThinkingSignature {
                            signature: sig.to_string(),
                        }));
                    }
                }
                _ => {}
            }
        }
        Some("message_stop") => {
            out.push(Ok(StreamEvent::MessageStop));
        }
        Some("error") => {
            out.push(Err(whycodes_core::Error::llm(
                event["error"]["message"]
                    .as_str()
                    .unwrap_or("Unknown error")
                    .to_string(),
            )));
        }
        _ => {}
    }
    out
}

fn content_block_to_anthropic(b: &ContentBlock) -> Value {
    match b {
        ContentBlock::Text { text } => serde_json::json!({"type": "text", "text": text}),
        ContentBlock::Image { source } => match source {
            whycodes_core::types::ImageSource::Base64 { media_type, data } => serde_json::json!({
                "type": "image",
                "source": {"type": "base64", "media_type": media_type, "data": data}
            }),
            _ => serde_json::json!({"type": "text", "text": "[image]"}),
        },
        ContentBlock::ToolUse { id, name, input } => {
            serde_json::json!({"type": "tool_use", "id": id, "name": name, "input": input})
        }
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
        ContentBlock::Thinking { text, signature } => {
            let mut v = serde_json::json!({"type": "thinking", "thinking": text});
            if let Some(sig) = signature.as_ref().filter(|s| !s.is_empty()) {
                v["signature"] = Value::String(sig.clone());
            }
            v
        }
        ContentBlock::RedactedThinking { data } => {
            serde_json::json!({"type": "redacted_thinking", "data": data})
        }
    }
}

pub struct AnthropicProvider {
    name: String,
    messages_url: String,
}

/// POST with the right auth header for the credential type.
///
/// OAuth subscription tokens (`sk-ant-oat…`, from `whycodes auth login
/// anthropic`) must go in `Authorization: Bearer` with the oauth beta flag;
/// plain API keys go in `x-api-key`. Sending an OAuth token as `x-api-key`
/// is rejected by the API.
///
/// Local proxies often have no credential — skip both headers when `api_key`
/// is empty rather than sending `x-api-key:`.
fn authed_post(url: &str, api_key: &str) -> reqwest::RequestBuilder {
    let req = crate::client_identity::post(url).header("anthropic-version", "2023-06-01");
    let key = api_key.trim();
    if key.is_empty() {
        req
    } else if key.starts_with("sk-ant-oat") {
        req.header("Authorization", format!("Bearer {key}"))
            .header("anthropic-beta", "oauth-2025-04-20")
    } else {
        req.header("x-api-key", key)
    }
}

impl AnthropicProvider {
    pub fn new() -> Self {
        Self::from_base(None)
    }

    pub fn from_config(config: &whycodes_core::types::ProviderConfig) -> Self {
        Self::from_base(config.base_url.as_deref().or(config.api_base.as_deref()))
    }

    pub fn from_base(base: Option<&str>) -> Self {
        Self {
            name: "anthropic".to_string(),
            messages_url: crate::endpoint::normalize_anthropic_messages_url(base),
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

        crate::openai_compat::apply_sampling(&mut body, request);

        crate::thinking::ThinkingConfig::apply_anthropic(&mut body, request.thinking.as_ref());

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
            .filter_map(|m| {
                let role = match m.role {
                    whycodes_core::types::Role::Assistant => "assistant",
                    whycodes_core::types::Role::User => "user",
                    whycodes_core::types::Role::System => "user", // system goes in top-level
                    whycodes_core::types::Role::Tool => "user",
                };

                let content: Vec<Value> = match &m.content {
                    whycodes_core::types::MessageContent::Text(text) => {
                        vec![serde_json::json!({"type": "text", "text": text})]
                    }
                    whycodes_core::types::MessageContent::Blocks(blocks) => {
                        let wire = if m.role == whycodes_core::types::Role::Assistant {
                            whycodes_core::types::strip_trailing_thinking(blocks)
                        } else {
                            blocks.clone()
                        };
                        wire.iter().map(content_block_to_anthropic).collect()
                    }
                };
                if content.is_empty() {
                    return None;
                }
                Some(serde_json::json!({"role": role, "content": content}))
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

impl LlmProvider for AnthropicProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_base_url(&self) -> &str {
        &self.messages_url
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
                                "thinking" => ContentBlock::Thinking {
                                    text: b["thinking"].as_str().unwrap_or("").to_string(),
                                    signature: b["signature"].as_str().map(str::to_string),
                                },
                                "redacted_thinking" => ContentBlock::RedactedThinking {
                                    data: b["data"].as_str().unwrap_or("").to_string(),
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
        })
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
        api_key: &'a str,
        model: &'a str,
    ) -> ProviderStreamFuture<'a> {
        Box::pin(async move {
            let body = self.build_body(request, model);
            let api_key = api_key.to_string();

            let resp =
                crate::oauth_refresh::send_with_refresh_retry(self.name(), &api_key, |key| {
                    authed_post(self.default_base_url(), key).json(&body)
                })
                .await?;

            if !resp.status().is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(whycodes_core::Error::llm(format!(
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

                                for event in events_for_data(data) {
                                    yield event;
                                }
                            }
                        }
                        Err(e) => {
                            yield Err(crate::openai_compat::stream_chunk_error("anthropic", e));
                        }
                    }
                }
            };

            Ok(Box::pin(s) as ProviderEventStream)
        })
    }
}

impl Default for AnthropicProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{AnthropicProvider, events_for_data, usage_from_message_delta};
    use serde_json::json;
    use whycodes_core::types::StreamEvent;

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

    #[test]
    fn data_message_start_emits_usage_and_cache_usage_when_present() {
        let events = events_for_data(
            r#"{"type":"message_start","message":{"usage":{"input_tokens":25,"cache_creation_input_tokens":5,"cache_read_input_tokens":7}}}"#,
        );
        assert_eq!(events.len(), 2, "{events:?}");
        assert!(matches!(
            &events[0],
            Ok(StreamEvent::Usage {
                input_tokens: 25,
                output_tokens: 0
            })
        ));
        assert!(matches!(
            &events[1],
            Ok(StreamEvent::CacheUsage {
                creation_input_tokens: 5,
                read_input_tokens: 7
            })
        ));
    }

    #[test]
    fn data_message_start_without_cache_tokens_skips_cache_event() {
        let events =
            events_for_data(r#"{"type":"message_start","message":{"usage":{"input_tokens":11}}}"#);
        assert_eq!(events.len(), 1, "{events:?}");
        assert!(matches!(
            &events[0],
            Ok(StreamEvent::Usage {
                input_tokens: 11,
                ..
            })
        ));
    }

    #[test]
    fn data_message_delta_emits_stop_reason_then_usage() {
        let events = events_for_data(
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"input_tokens":40,"output_tokens":12}}"#,
        );
        assert_eq!(events.len(), 2, "{events:?}");
        assert!(matches!(&events[0], Ok(StreamEvent::MessageDelta { .. })));
        assert!(matches!(
            &events[1],
            Ok(StreamEvent::Usage {
                input_tokens: 40,
                output_tokens: 12
            })
        ));
    }

    #[test]
    fn data_content_block_start_tool_use_carries_id_name_input() {
        let events = events_for_data(
            r#"{"type":"content_block_start","content_block":{"type":"tool_use","id":"tu_1","name":"read_file","input":{"path":"a.rs"}}}"#,
        );
        assert_eq!(events.len(), 1, "{events:?}");
        assert!(matches!(
            &events[0],
            Ok(StreamEvent::ToolUse { id, name, .. }) if id == "tu_1" && name == "read_file"
        ));
    }

    #[test]
    fn data_content_block_start_thinking_emits_text_and_signature() {
        let events = events_for_data(
            r#"{"type":"content_block_start","content_block":{"type":"thinking","thinking":"hmm","signature":"sig9"}}"#,
        );
        assert_eq!(events.len(), 2, "{events:?}");
        assert!(matches!(&events[0], Ok(StreamEvent::Thinking { text } ) if text == "hmm"));
        assert!(
            matches!(&events[1], Ok(StreamEvent::ThinkingSignature { signature } ) if signature == "sig9")
        );
    }

    #[test]
    fn data_content_block_start_redacted_thinking_passes_data() {
        let events = events_for_data(
            r#"{"type":"content_block_start","content_block":{"type":"redacted_thinking","data":"opaque"}}"#,
        );
        assert_eq!(events.len(), 1, "{events:?}");
        assert!(
            matches!(&events[0], Ok(StreamEvent::RedactedThinking { data } ) if data == "opaque")
        );
    }

    #[test]
    fn data_content_block_start_unknown_type_is_silent() {
        let events = events_for_data(
            r#"{"type":"content_block_start","content_block":{"type":"server_tool_use"}}"#,
        );
        assert!(events.is_empty());
    }

    #[test]
    fn data_content_block_delta_covers_all_four_delta_kinds() {
        let text = events_for_data(
            r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"hi"}}"#,
        );
        assert!(matches!(&text[0], Ok(StreamEvent::TextDelta { text }) if text == "hi"));

        let json = events_for_data(
            r#"{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{\"a\":1}"}}"#,
        );
        assert!(matches!(
            &json[0],
            Ok(StreamEvent::ToolUseDelta { input_json_delta, .. }) if input_json_delta == "{\"a\":1}"
        ));

        let think = events_for_data(
            r#"{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"t"}}"#,
        );
        assert!(matches!(&think[0], Ok(StreamEvent::ThinkingDelta { text }) if text == "t"));

        let sig = events_for_data(
            r#"{"type":"content_block_delta","delta":{"type":"signature_delta","signature":"s"}}"#,
        );
        assert!(
            matches!(&sig[0], Ok(StreamEvent::ThinkingSignature { signature }) if signature == "s")
        );
    }

    #[test]
    fn data_message_stop_and_error_are_mapped() {
        let stop = events_for_data(r#"{"type":"message_stop"}"#);
        assert!(matches!(stop[0], Ok(StreamEvent::MessageStop)));

        let err = events_for_data(r#"{"type":"error","error":{"message":"overloaded"}}"#);
        assert!(err[0].is_err());
        assert!(
            err[0]
                .as_ref()
                .unwrap_err()
                .to_string()
                .contains("overloaded")
        );
    }

    #[test]
    fn data_invalid_json_and_unknown_types_yield_nothing() {
        assert!(events_for_data("not json at all").is_empty());
        assert!(events_for_data(r#"{"type":"ping"}"#).is_empty());
    }

    use std::sync::Arc;
    use whycodes_core::types::{
        ContentBlock, ImageSource, LlmRequest, Message, MessageContent, Role, ToolDefinition,
    };

    fn base_request() -> LlmRequest {
        LlmRequest {
            system: String::new(),
            messages: Arc::from(vec![]),
            tools: std::sync::Arc::from([]),
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            thinking: None,
            use_prompt_cache: false,
        }
    }

    #[test]
    fn prompt_cache_promotes_system_to_ephemeral_block() {
        let provider = AnthropicProvider::new();
        let mut req = base_request();
        req.use_prompt_cache = true;
        req.system = "sys".into();
        let body = provider.build_body(&req, "m");
        let system = body["system"].as_array().expect("cached system");
        assert_eq!(system[0]["type"], "text");
        assert_eq!(system[0]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn build_body_defaults_options_and_tool_shape() {
        let provider = AnthropicProvider::new();
        let mut req = base_request();
        req.system = "sys".into();
        req.max_tokens = Some(100);
        req.temperature = Some(0.5);
        req.top_p = Some(0.25);
        req.tools = vec![ToolDefinition {
            name: "read".into(),
            description: "Read a file".into(),
            parameters: json!({"type": "object"}),
        }]
        .into();

        let body = provider.build_body(&req, "claude-sonnet-4");
        assert_eq!(body["model"], "claude-sonnet-4");
        assert_eq!(body["max_tokens"], 100);
        assert_eq!(body["stream"], true);
        assert_eq!(body["system"], "sys");
        assert_eq!(body["temperature"], 0.5);
        assert_eq!(body["top_p"], 0.25);

        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "read");
        assert_eq!(tools[0]["input_schema"], json!({"type": "object"}));
    }

    #[test]
    fn build_body_omits_absent_optionals() {
        let provider = AnthropicProvider::new();
        let body = provider.build_body(&base_request(), "m");
        assert_eq!(body["max_tokens"], 4096);
        assert!(body.get("system").is_none());
        assert!(body.get("tools").is_none());
        assert!(body.get("temperature").is_none());
        assert!(body.get("top_p").is_none());
    }

    #[test]
    fn convert_messages_maps_roles_blocks_and_drops_empty() {
        let provider = AnthropicProvider::new();
        let mut req = base_request();
        req.messages = Arc::from(vec![
            Message {
                role: Role::System,
                content: MessageContent::Text("s".into()),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
            Message {
                role: Role::User,
                tool_call_id: None,
                name: None,
                created_at: None,
                content: MessageContent::Blocks(vec![
                    ContentBlock::Text { text: "hi".into() },
                    ContentBlock::Image {
                        source: ImageSource::Base64 {
                            media_type: "image/png".into(),
                            data: "AAAA".into(),
                        },
                    },
                    ContentBlock::Image {
                        source: ImageSource::Url {
                            url: "https://x/y.png".into(),
                        },
                    },
                    ContentBlock::ToolResult {
                        tool_use_id: "t1".into(),
                        content: "out".into(),
                        is_error: None,
                    },
                ]),
            },
            Message {
                role: Role::Assistant,
                tool_call_id: None,
                name: None,
                created_at: None,
                content: MessageContent::Blocks(vec![
                    ContentBlock::Thinking {
                        text: "hmm".into(),
                        signature: Some("sig".into()),
                    },
                    ContentBlock::RedactedThinking { data: "opq".into() },
                    ContentBlock::ToolUse {
                        id: "t1".into(),
                        name: "read".into(),
                        input: json!({"path": "a.rs"}),
                    },
                ]),
            },
            Message {
                role: Role::Tool,
                content: MessageContent::Text("result".into()),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
            Message {
                role: Role::Assistant,
                tool_call_id: None,
                name: None,
                created_at: None,
                content: MessageContent::Blocks(vec![ContentBlock::Thinking {
                    text: "only thinking".into(),
                    signature: None,
                }]),
            },
        ]);

        let body = provider.build_body(&req, "m");
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 4, "{msgs:?}");

        assert_eq!(msgs[0]["role"], "user");

        let user_blocks = msgs[1]["content"].as_array().unwrap();
        assert_eq!(user_blocks[0]["type"], "text");
        assert_eq!(user_blocks[1]["type"], "image");
        assert_eq!(user_blocks[1]["source"]["media_type"], "image/png");
        assert_eq!(user_blocks[2]["type"], "text", "url image degrades to text");
        assert_eq!(user_blocks[3]["type"], "tool_result");
        assert_eq!(user_blocks[3]["is_error"], false);

        let a_blocks = msgs[2]["content"].as_array().unwrap();
        assert_eq!(a_blocks[0]["type"], "thinking");
        assert_eq!(a_blocks[0]["signature"], "sig");
        assert_eq!(a_blocks[1]["type"], "redacted_thinking");
        assert_eq!(a_blocks[2]["type"], "tool_use");

        assert_eq!(msgs[3]["role"], "user", "tool role maps to user");
    }

    #[test]
    fn thinking_without_signature_omits_the_field() {
        let provider = AnthropicProvider::new();
        let mut req = base_request();
        req.messages = Arc::from(vec![Message {
            role: Role::Assistant,
            tool_call_id: None,
            name: None,
            created_at: None,
            content: MessageContent::Blocks(vec![
                ContentBlock::Thinking {
                    text: "t".into(),
                    signature: None,
                },
                ContentBlock::Text {
                    text: "answer".into(),
                },
            ]),
        }]);
        let body = provider.build_body(&req, "m");
        let block = &body["messages"][0]["content"][0];
        assert_eq!(block["type"], "thinking");
        assert!(block.get("signature").is_none());
    }
}
