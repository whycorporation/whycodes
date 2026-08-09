//! ChatGPT-subscription call routing: the Codex backend.
//!
//! A ChatGPT Plus/Pro OAuth token (`whycode auth login openai`) is rejected
//! by `api.openai.com`; it only authorizes the Codex backend at
//! `chatgpt.com/backend-api`. This module speaks the Responses API against
//! that endpoint. `OpenAiProvider` delegates here when the credential is
//! JWT-shaped (see [`is_chatgpt_oauth_token`]), so API keys keep the
//! chat-completions path untouched.
//!
//! The backend only serves streaming responses (`store: false`,
//! `stream: true`), so [`complete`] assembles its answer from the same SSE
//! stream. The OAuth login rides on the public Codex CLI client id
//! (docs/auth.md); requests use the matching `originator` identity.

use async_stream::stream;
use futures::stream::{Stream, StreamExt};
use serde_json::{Value, json};
use std::pin::Pin;
use whycode_core::types::{
    ContentBlock, ImageSource, LlmRequest, LlmResponse, Message, MessageContent, Role, StreamEvent,
    ToolDefinition, Usage,
};

/// Responses-API endpoint of the ChatGPT backend that Codex-client
/// subscription tokens authorize.
pub const CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";

/// The backend gates client features on this header; the login flow already
/// uses the public Codex CLI client id, so calls present the same family.
const ORIGINATOR: &str = "codex_cli_rs";

/// True when `key` is a ChatGPT-subscription OAuth access token (a JWT)
/// rather than an OpenAI API key (`sk-…`). JWTs are rejected by
/// api.openai.com and must be routed to the Codex backend.
pub fn is_chatgpt_oauth_token(key: &str) -> bool {
    key.starts_with("eyJ") && key.matches('.').count() == 2
}

/// Responses-API request body. The backend mandates `store: false` +
/// `stream: true`; the system prompt travels as `instructions`.
pub fn build_body(request: &LlmRequest, model: &str) -> Value {
    let mut body = json!({
        "model": model,
        "store": false,
        "stream": true,
        "input": convert_input(&request.messages),
    });
    if !request.system.is_empty() {
        body["instructions"] = Value::String(request.system.clone());
    }
    if !request.tools.is_empty() {
        body["tools"] = Value::Array(convert_tools(&request.tools));
        body["tool_choice"] = json!("auto");
        body["parallel_tool_calls"] = json!(true);
    }
    body
}

/// Convert the message history to Responses-API input items, preserving
/// order: text → `message`, tool calls → `function_call`, results →
/// `function_call_output` (matched by `call_id`).
fn convert_input(messages: &[Message]) -> Vec<Value> {
    let mut items = Vec::new();
    for m in messages {
        let role = match m.role {
            Role::Assistant => "assistant",
            // System/Tool roles reach us as ordinary messages; the real
            // system prompt is the top-level `instructions` field.
            _ => "user",
        };
        match &m.content {
            MessageContent::Text(text) => {
                if !text.trim().is_empty() {
                    items.push(message_item(role, text));
                }
            }
            MessageContent::Blocks(blocks) => {
                let mut texts: Vec<&str> = Vec::new();
                for b in blocks {
                    match b {
                        ContentBlock::Text { text } => texts.push(text),
                        ContentBlock::Image { source } => {
                            flush_message(&mut items, role, &mut texts);
                            if let Some(part) = image_part(role, source) {
                                items.push(json!({
                                    "type": "message", "role": role, "content": [part]
                                }));
                            }
                        }
                        ContentBlock::ToolUse { id, name, input } => {
                            flush_message(&mut items, role, &mut texts);
                            items.push(json!({
                                "type": "function_call",
                                "call_id": id,
                                "name": name,
                                "arguments": input.to_string(),
                            }));
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } => {
                            flush_message(&mut items, role, &mut texts);
                            items.push(json!({
                                "type": "function_call_output",
                                "call_id": tool_use_id,
                                "output": content,
                            }));
                        }
                    }
                }
                flush_message(&mut items, role, &mut texts);
            }
        }
    }
    items
}

fn flush_message(items: &mut Vec<Value>, role: &str, texts: &mut Vec<&str>) {
    if texts.is_empty() {
        return;
    }
    let joined = texts.join("\n");
    items.push(message_item(role, &joined));
    texts.clear();
}

fn message_item(role: &str, text: &str) -> Value {
    let part_type = if role == "assistant" {
        "output_text"
    } else {
        "input_text"
    };
    json!({
        "type": "message",
        "role": role,
        "content": [{ "type": part_type, "text": text }],
    })
}

/// Responses takes images as data URLs inside `input_image` parts.
fn image_part(role: &str, source: &ImageSource) -> Option<Value> {
    if role != "user" {
        return None;
    }
    let url = match source {
        ImageSource::Base64 { media_type, data } => format!("data:{media_type};base64,{data}"),
        ImageSource::Url { url } => url.clone(),
    };
    Some(json!({ "type": "input_image", "image_url": url }))
}

fn convert_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "name": t.name,
                "description": t.description,
                "parameters": t.parameters,
            })
        })
        .collect()
}

/// POST to the Codex backend with one OAuth-aware retry: a 401 force-renews
/// the stored credential via `oauth_refresh` and resends once.
async fn post(api_key: &str, body: &Value) -> whycode_core::Result<reqwest::Response> {
    let account_id = super::oauth_refresh::stored_extra("openai", "openai_account_id").await;
    super::oauth_refresh::send_with_refresh_retry("openai", api_key, |key| {
        let req = super::client_identity::post(CODEX_RESPONSES_URL)
            .header("Authorization", format!("Bearer {key}"))
            .header("OpenAI-Beta", "responses=experimental")
            .header("Accept", "text/event-stream")
            .header("originator", ORIGINATOR);
        let req = match &account_id {
            Some(id) => req.header("chatgpt-account-id", id),
            None => req,
        };
        req.json(body)
    })
    .await
}

/// Map one Responses-API SSE `data:` payload to whycode stream events.
/// Pure so the event dialect is unit-testable without a network.
fn events_for_payload(data: &str) -> Vec<StreamEvent> {
    let Ok(event) = serde_json::from_str::<Value>(data) else {
        return Vec::new();
    };
    match event["type"].as_str() {
        Some("response.output_text.delta") => event["delta"]
            .as_str()
            .map(|d| {
                vec![StreamEvent::TextDelta {
                    text: d.to_string(),
                }]
            })
            .unwrap_or_default(),
        Some("response.output_item.done") if event["item"]["type"] == "function_call" => {
            let item = &event["item"];
            let input = item["arguments"]
                .as_str()
                .and_then(|a| serde_json::from_str(a).ok())
                .unwrap_or(Value::Null);
            vec![StreamEvent::ToolUse {
                id: item["call_id"].as_str().unwrap_or_default().to_string(),
                name: item["name"].as_str().unwrap_or_default().to_string(),
                input,
            }]
        }
        Some("response.completed") => {
            let usage = &event["response"]["usage"];
            let mut events = Vec::new();
            let cached = usage["input_tokens_details"]["cached_tokens"]
                .as_u64()
                .unwrap_or(0);
            if cached > 0 {
                events.push(StreamEvent::CacheUsage {
                    creation_input_tokens: 0,
                    read_input_tokens: cached,
                });
            }
            events.push(StreamEvent::Usage {
                input_tokens: usage["input_tokens"].as_u64().unwrap_or(0),
                output_tokens: usage["output_tokens"].as_u64().unwrap_or(0),
            });
            events.push(StreamEvent::MessageStop);
            events
        }
        Some("response.failed") | Some("error") => {
            let msg = event["response"]["error"]["message"]
                .as_str()
                .or_else(|| event["message"].as_str())
                .unwrap_or("Codex backend stream failed");
            vec![StreamEvent::Error {
                message: msg.to_string(),
            }]
        }
        _ => Vec::new(),
    }
}

pub async fn complete(
    request: &LlmRequest,
    api_key: &str,
    model: &str,
) -> whycode_core::Result<LlmResponse> {
    let mut events = stream(request, api_key, model).await?;
    let mut content: Vec<ContentBlock> = Vec::new();
    let mut text = String::new();
    let mut stop_reason = None;
    let mut usage = Usage {
        input_tokens: 0,
        output_tokens: 0,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    };
    while let Some(event) = events.next().await {
        match event? {
            StreamEvent::TextDelta { text: delta } => text.push_str(&delta),
            StreamEvent::ToolUse { id, name, input } => {
                if !text.is_empty() {
                    content.push(ContentBlock::Text {
                        text: std::mem::take(&mut text),
                    });
                }
                content.push(ContentBlock::ToolUse { id, name, input });
            }
            StreamEvent::Usage {
                input_tokens,
                output_tokens,
            } => {
                usage.input_tokens = input_tokens;
                usage.output_tokens = output_tokens;
            }
            StreamEvent::CacheUsage {
                read_input_tokens, ..
            } => {
                usage.cache_read_input_tokens = Some(read_input_tokens);
            }
            StreamEvent::MessageStop => stop_reason = Some("stop".to_string()),
            _ => {}
        }
    }
    if !text.is_empty() {
        content.push(ContentBlock::Text { text });
    }
    Ok(LlmResponse {
        content,
        stop_reason,
        usage,
        model: model.to_string(),
    })
}

pub async fn stream(
    request: &LlmRequest,
    api_key: &str,
    model: &str,
) -> whycode_core::Result<Pin<Box<dyn Stream<Item = whycode_core::Result<StreamEvent>> + Send>>> {
    let body = build_body(request, model);
    let resp = post(api_key, &body).await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let trimmed: String = text.chars().take(500).collect();
        return Err(whycode_core::Error::Llm(format!(
            "Codex backend error ({status}): {trimmed}"
        )));
    }

    let s = stream! {
        let mut byte_stream = resp.bytes_stream();
        let mut buffer = String::new();
        let mut stopped = false;

        while let Some(chunk) = byte_stream.next().await {
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
                            break;
                        }
                        for ev in events_for_payload(data) {
                            if matches!(ev, StreamEvent::MessageStop) {
                                stopped = true;
                            }
                            yield Ok(ev);
                        }
                        if stopped {
                            return;
                        }
                    }
                }
                Err(e) => {
                    yield Err(whycode_core::Error::Llm(format!("Stream error: {e}")));
                }
            }
        }
        // The backend closes the connection after `response.completed`;
        // tolerate a missing one so the agent still finishes its turn.
        if !stopped {
            yield Ok(StreamEvent::MessageStop);
        }
    };

    Ok(Box::pin(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jwt_shape_detection() {
        assert!(is_chatgpt_oauth_token("eyJhbGciOiJ.eyJzdWIiOiJx.sig"));
        assert!(!is_chatgpt_oauth_token("sk-proj-abc123"));
        assert!(!is_chatgpt_oauth_token("sk-ant-oat01-xyz"));
        assert!(!is_chatgpt_oauth_token("eyJnoDots"));
        assert!(!is_chatgpt_oauth_token(""));
    }

    fn request_with_tools() -> LlmRequest {
        LlmRequest {
            system: "You are whycode.".to_string(),
            messages: vec![
                Message {
                    role: Role::User,
                    content: MessageContent::Text("hi".to_string()),
                    tool_call_id: None,
                    name: None,
                },
                Message {
                    role: Role::Assistant,
                    tool_call_id: None,
                    name: None,
                    content: MessageContent::Blocks(vec![
                        ContentBlock::Text {
                            text: "let me check".to_string(),
                        },
                        ContentBlock::ToolUse {
                            id: "call_1".to_string(),
                            name: "read".to_string(),
                            input: json!({"path": "a.rs"}),
                        },
                    ]),
                },
                Message {
                    role: Role::User,
                    tool_call_id: None,
                    name: None,
                    content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                        tool_use_id: "call_1".to_string(),
                        content: "fn main() {}".to_string(),
                        is_error: Some(false),
                    }]),
                },
            ],
            tools: vec![ToolDefinition {
                name: "read".to_string(),
                description: "Read a file".to_string(),
                parameters: json!({"type": "object"}),
            }],
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            thinking: None,
            use_prompt_cache: true,
        }
    }

    #[test]
    fn body_matches_backend_contract() {
        let body = build_body(&request_with_tools(), "gpt-5.1-codex");
        assert_eq!(body["model"], "gpt-5.1-codex");
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
        assert_eq!(body["instructions"], "You are whycode.");
        assert_eq!(body["tool_choice"], "auto");
        assert_eq!(body["parallel_tool_calls"], true);
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "read");
        assert_eq!(body["tools"][0]["parameters"]["type"], "object");

        let input = body["input"].as_array().unwrap();
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"][0]["type"], "input_text");
        assert_eq!(input[1]["role"], "assistant");
        assert_eq!(input[1]["content"][0]["type"], "output_text");
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[2]["call_id"], "call_1");
        assert_eq!(input[2]["name"], "read");
        // arguments travel as a JSON *string*.
        assert_eq!(
            input[2]["arguments"].as_str().unwrap(),
            json!({"path": "a.rs"}).to_string()
        );
        assert_eq!(input[3]["type"], "function_call_output");
        assert_eq!(input[3]["call_id"], "call_1");
        assert_eq!(input[3]["output"], "fn main() {}");
    }

    #[test]
    fn text_delta_maps_to_stream_event() {
        let events = events_for_payload(r#"{"type":"response.output_text.delta","delta":"hel"}"#);
        assert_eq!(events.len(), 1);
        match &events[0] {
            StreamEvent::TextDelta { text } => assert_eq!(text, "hel"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn function_call_item_maps_to_tool_use() {
        let payload = r#"{"type":"response.output_item.done","item":{"type":"function_call","call_id":"call_9","name":"bash","arguments":"{\"cmd\":\"ls\"}"}}"#;
        let events = events_for_payload(payload);
        assert_eq!(events.len(), 1);
        match &events[0] {
            StreamEvent::ToolUse { id, name, input } => {
                assert_eq!(id, "call_9");
                assert_eq!(name, "bash");
                assert_eq!(input["cmd"], "ls");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn completed_maps_usage_cache_and_stop() {
        let payload = r#"{"type":"response.completed","response":{"usage":{"input_tokens":10,"output_tokens":4,"input_tokens_details":{"cached_tokens":6}}}}"#;
        let events = events_for_payload(payload);
        assert!(matches!(
            events[0],
            StreamEvent::CacheUsage {
                read_input_tokens: 6,
                ..
            }
        ));
        assert!(matches!(
            events[1],
            StreamEvent::Usage {
                input_tokens: 10,
                output_tokens: 4
            }
        ));
        assert!(matches!(events[2], StreamEvent::MessageStop));
    }

    #[test]
    fn failed_maps_to_error_event() {
        let payload = r#"{"type":"response.failed","response":{"error":{"message":"boom"}}}"#;
        let events = events_for_payload(payload);
        assert!(matches!(&events[0], StreamEvent::Error { message } if message == "boom"));
    }

    #[test]
    fn unknown_events_are_ignored() {
        assert!(events_for_payload(r#"{"type":"response.created"}"#).is_empty());
        assert!(events_for_payload("not json").is_empty());
    }
}
