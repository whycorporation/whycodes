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
use futures::stream::{Stream, StreamExt};
use serde_json::Value;
use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};
use whycodes_core::types::{
    ContentBlock, ImageSource, LlmRequest, LlmResponse, Message, MessageContent, Role, StreamEvent,
    ToolDefinition, Usage,
};

/// Responses-API endpoint of the ChatGPT backend that Codex-client
/// subscription tokens authorize.
pub const CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";

#[cfg(test)]
thread_local! {
    pub(crate) static TEST_CODEX_URL: std::cell::RefCell<Option<&'static str>> =
        const { std::cell::RefCell::new(None) };
}

fn codex_responses_url() -> &'static str {
    #[cfg(test)]
    if let Some(url) = TEST_CODEX_URL.with(|c| *c.borrow()) {
        return url;
    }
    CODEX_RESPONSES_URL
}

/// True when `key` is a ChatGPT-subscription OAuth access token (a JWT)
/// rather than an OpenAI API key (`sk-…`). JWTs are rejected by
/// api.openai.com and must be routed to the Codex backend.
pub fn is_chatgpt_oauth_token(key: &str) -> bool {
    key.starts_with("eyJ") && key.matches('.').count() == 2
}

/// Responses-API request body. The backend mandates `store: false` +
/// `stream: true`; the system prompt travels as `instructions`.
pub fn build_body(request: &LlmRequest, model: &str) -> Value {
    let mut body = crate::json_value::obj([
        ("model", crate::json_value::str(model)),
        ("store", Value::Bool(false)),
        ("stream", Value::Bool(true)),
        (
            "input",
            crate::json_value::arr(convert_input(&request.messages)),
        ),
    ]);
    if !request.system.is_empty() {
        crate::json_value::insert(
            &mut body,
            "instructions",
            crate::json_value::str(&request.system),
        );
    }
    if !request.tools.is_empty() {
        crate::json_value::insert(
            &mut body,
            "tools",
            crate::json_value::arr(convert_tools(&request.tools)),
        );
        crate::json_value::insert(&mut body, "tool_choice", crate::json_value::str("auto"));
        crate::json_value::insert(&mut body, "parallel_tool_calls", Value::Bool(true));
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
                                items.push(crate::json_value::obj([
                                    ("type", crate::json_value::str("message")),
                                    ("role", crate::json_value::str(role)),
                                    ("content", crate::json_value::arr([part])),
                                ]));
                            }
                        }
                        ContentBlock::ToolUse { id, name, input } => {
                            flush_message(&mut items, role, &mut texts);
                            items.push(crate::json_value::obj([
                                ("type", crate::json_value::str("function_call")),
                                ("call_id", crate::json_value::str(id)),
                                ("name", crate::json_value::str(name)),
                                ("arguments", crate::json_value::str(input.to_string())),
                            ]));
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } => {
                            flush_message(&mut items, role, &mut texts);
                            items.push(crate::json_value::obj([
                                ("type", crate::json_value::str("function_call_output")),
                                ("call_id", crate::json_value::str(tool_use_id)),
                                ("output", crate::json_value::str(content)),
                            ]));
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
    crate::json_value::obj([
        ("type", crate::json_value::str("message")),
        ("role", crate::json_value::str(role)),
        (
            "content",
            crate::json_value::arr([crate::json_value::obj([
                ("type", crate::json_value::str(part_type)),
                ("text", crate::json_value::str(text)),
            ])]),
        ),
    ])
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
    Some(crate::json_value::obj([
        ("type", crate::json_value::str("input_image")),
        ("image_url", crate::json_value::str(url)),
    ]))
}

fn convert_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            crate::json_value::obj([
                ("type", crate::json_value::str("function")),
                ("name", crate::json_value::str(&t.name)),
                ("description", crate::json_value::str(&t.description)),
                ("parameters", t.parameters.clone()),
            ])
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
        let req = if url == CODEX_RESPONSES_URL || url == codex_responses_url() {
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
    complete_at(codex_responses_url(), request, api_key, model).await
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
    stream_at(codex_responses_url(), request, api_key, model).await
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

    Ok(Box::pin(CodexSse {
        bytes: crate::openai_compat::response_bytes(resp),
        buffer: String::new(),
        pending: VecDeque::new(),
        done: false,
    }) as ProviderEventStream)
}

struct CodexSse {
    bytes: crate::openai_compat::ByteStream,
    buffer: String,
    pending: VecDeque<whycodes_core::Result<StreamEvent>>,
    done: bool,
}

impl CodexSse {
    fn push_data_line(&mut self, line: &str) {
        if line.is_empty() || !line.starts_with("data: ") {
            return;
        }
        let data = &line[6..];
        if data == "[DONE]" {
            if !self.done {
                self.pending.push_back(Ok(StreamEvent::MessageStop));
                self.done = true;
            }
            return;
        }
        for ev in events_for_payload(data) {
            if matches!(ev, StreamEvent::MessageStop) {
                self.done = true;
            }
            self.pending.push_back(Ok(ev));
        }
    }
}

impl Stream for CodexSse {
    type Item = whycodes_core::Result<StreamEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(ev) = this.pending.pop_front() {
                return Poll::Ready(Some(ev));
            }
            if this.done {
                return Poll::Ready(None);
            }
            match this.bytes.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    this.buffer.push_str(&String::from_utf8_lossy(&bytes));
                    while let Some(pos) = this.buffer.find('\n') {
                        let line = this.buffer[..pos].trim().to_string();
                        this.buffer = this.buffer[pos + 1..].to_string();
                        this.push_data_line(&line);
                        if this.done {
                            break;
                        }
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(crate::openai_compat::stream_chunk_error(
                        "openai-codex",
                        e,
                    ))));
                }
                Poll::Ready(None) => {
                    // The backend closes the connection after `response.completed`;
                    // tolerate a missing one so the agent still finishes its turn.
                    if !this.done {
                        this.pending.push_back(Ok(StreamEvent::MessageStop));
                        this.done = true;
                    }
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[cfg(test)]
#[path = "codex_tests.rs"]
mod tests;
