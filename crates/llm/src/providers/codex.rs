//! ChatGPT-subscription call routing: the Codex backend.
//!
//! A ChatGPT Plus/Pro OAuth token (`whycodes auth login openai`) is rejected
//! by `api.openai.com`; it only authorizes the Codex backend at
//! `chatgpt.com/backend-api`. This module speaks the Responses API against
//! that endpoint. `OpenAiProvider` delegates here when the credential is
//! JWT-shaped (see [`is_chatgpt_oauth_token`]), so API keys keep the
//! chat-completions path untouched.
//!
//! The backend only serves streaming responses (`store: false`,
//! `stream: true`), so [`complete`] assembles its answer from the same SSE
//! stream. Core traffic identifies as WhyCodes; an unofficial auth plugin
//! may attach an `originator` header via `inference.headers`.

use crate::provider::ProviderEventStream;
use async_stream::stream;
use futures::stream::{Stream, StreamExt};
use serde_json::{Value, json};
use std::pin::Pin;
use whycodes_core::types::{
    ContentBlock, ImageSource, LlmRequest, LlmResponse, Message, MessageContent, Role, StreamEvent,
    ToolDefinition, Usage,
};

/// Responses-API endpoint of the ChatGPT backend that Codex-client
/// subscription tokens authorize.
pub const CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";

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
                        ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. } => {}
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
async fn post_at(
    url: &str,
    api_key: &str,
    body: &Value,
) -> whycodes_core::Result<reqwest::Response> {
    let account_id = crate::oauth_refresh::stored_extra("openai", "openai_account_id").await;
    crate::oauth_refresh::send_with_refresh_retry("openai", api_key, |key| {
        let req = if url == CODEX_RESPONSES_URL {
            crate::client_identity::post_for_provider(url, "openai")
        } else {
            crate::client_identity::post(url)
        }
        .header("Authorization", format!("Bearer {key}"))
        .header("OpenAI-Beta", "responses=experimental")
        .header("Accept", "text/event-stream");
        let req = match &account_id {
            Some(id) => req.header("chatgpt-account-id", id),
            None => req,
        };
        req.json(body)
    })
    .await
}

/// Map one Responses-API SSE `data:` payload to whycodes stream events.
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
            crate::usage_dump::dump_raw_usage("codex", usage);
            // OpenAI-style `input_tokens_details.cached_tokens` is a **subset**
            // of `input_tokens`, not Anthropic additive cache. Emitting
            // CacheUsage here double-counted in `Usage::total()` and the
            // context meter. Leave cache fields unset (same as chat.completions).
            vec![
                StreamEvent::Usage {
                    input_tokens: usage["input_tokens"].as_u64().unwrap_or(0),
                    output_tokens: usage["output_tokens"].as_u64().unwrap_or(0),
                },
                StreamEvent::MessageStop,
            ]
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
) -> whycodes_core::Result<LlmResponse> {
    complete_at(CODEX_RESPONSES_URL, request, api_key, model).await
}

pub(crate) async fn complete_at(
    url: &str,
    request: &LlmRequest,
    api_key: &str,
    model: &str,
) -> whycodes_core::Result<LlmResponse> {
    let mut events = stream_at(url, request, api_key, model).await?;
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
) -> whycodes_core::Result<Pin<Box<dyn Stream<Item = whycodes_core::Result<StreamEvent>> + Send>>> {
    stream_at(CODEX_RESPONSES_URL, request, api_key, model).await
}

pub(crate) async fn stream_at(
    url: &str,
    request: &LlmRequest,
    api_key: &str,
    model: &str,
) -> whycodes_core::Result<Pin<Box<dyn Stream<Item = whycodes_core::Result<StreamEvent>> + Send>>> {
    let body = build_body(request, model);
    let resp = post_at(url, api_key, &body).await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let trimmed: String = text.chars().take(500).collect();
        return Err(whycodes_core::Error::llm(format!(
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
                    yield Err(crate::openai_compat::stream_chunk_error("openai-codex", e));
                }
            }
        }
        // The backend closes the connection after `response.completed`;
        // tolerate a missing one so the agent still finishes its turn.
        if !stopped {
            yield Ok(StreamEvent::MessageStop);
        }
    };

    Ok(Box::pin(s) as ProviderEventStream)
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
            system: "You are whycodes.".to_string(),
            messages: std::sync::Arc::from(vec![
                Message {
                    role: Role::User,
                    content: MessageContent::Text("hi".to_string()),
                    tool_call_id: None,
                    name: None,
                    created_at: None,
                },
                Message {
                    role: Role::Assistant,
                    tool_call_id: None,
                    name: None,
                    created_at: None,
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
                    created_at: None,
                    content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                        tool_use_id: "call_1".to_string(),
                        content: "fn main() {}".to_string(),
                        is_error: Some(false),
                    }]),
                },
            ]),
            tools: vec![ToolDefinition {
                name: "read".to_string(),
                description: "Read a file".to_string(),
                parameters: json!({"type": "object"}),
            }]
            .into(),
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
        assert_eq!(body["instructions"], "You are whycodes.");
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
    fn completed_maps_usage_without_openai_subset_cache() {
        let payload = r#"{"type":"response.completed","response":{"usage":{"input_tokens":10,"output_tokens":4,"input_tokens_details":{"cached_tokens":6}}}}"#;
        let events = events_for_payload(payload);
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            StreamEvent::Usage {
                input_tokens: 10,
                output_tokens: 4
            }
        ));
        assert!(matches!(events[1], StreamEvent::MessageStop));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, StreamEvent::CacheUsage { .. })),
            "cached_tokens is inside input_tokens — must not become additive CacheUsage"
        );
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

    fn serve_sse(status: &str, body: &str) -> String {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;
        use std::time::Duration;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let header = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let payload = format!("{header}{body}");
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(payload.as_bytes());
                thread::sleep(Duration::from_millis(20));
            }
        });
        format!("http://{addr}/codex/responses")
    }

    fn simple_request() -> LlmRequest {
        LlmRequest {
            system: String::new(),
            messages: std::sync::Arc::from(vec![Message {
                role: Role::User,
                content: MessageContent::Text("hi".into()),
                tool_call_id: None,
                name: None,
                created_at: None,
            }]),
            tools: vec![].into(),
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            thinking: None,
            use_prompt_cache: false,
        }
    }

    fn sse_hello() -> String {
        format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            serde_json::json!({"type":"response.output_text.delta","delta":"hello"}),
            serde_json::json!({
                "type":"response.completed",
                "response":{"usage":{"input_tokens":1,"output_tokens":2}}
            })
        )
    }

    #[tokio::test]
    async fn complete_and_stream_against_loopback() {
        let url = serve_sse("200 OK", &sse_hello());
        let req = simple_request();
        let resp = complete_at(&url, &req, "eyJhbGciOiJ.eyJzdWIiOiJx.sig", "gpt-test")
            .await
            .unwrap();
        assert!(
            resp.content
                .iter()
                .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("hello"))),
            "{resp:?}"
        );
        assert_eq!(resp.usage.input_tokens, 1);
        assert_eq!(resp.usage.output_tokens, 2);

        let err_url = serve_sse("401 Unauthorized", "nope");
        let err = complete_at(&err_url, &req, "eyJhbGciOiJ.eyJzdWIiOiJx.sig", "gpt-test")
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("401") || !err.to_string().is_empty(),
            "{err}"
        );

        let stream_url = serve_sse("200 OK", &sse_hello());
        let mut stream = stream_at(
            &stream_url,
            &req,
            "eyJhbGciOiJ.eyJzdWIiOiJx.sig",
            "gpt-test",
        )
        .await
        .unwrap();
        let mut text = String::new();
        while let Some(ev) = stream.next().await {
            if let Ok(StreamEvent::TextDelta { text: d }) = ev {
                text.push_str(&d);
            }
        }
        assert_eq!(text, "hello");
    }
}
