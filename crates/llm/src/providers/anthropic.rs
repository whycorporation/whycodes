/// Anthropic Claude LLM provider implementation.
/// Supports streaming with extended thinking via the Anthropic Messages API.
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::Stream;
use serde_json::Value;
use whycodes_core::types::{
    ContentBlock, LlmRequest, LlmResponse, Message, StreamEvent, ToolDefinition, Usage,
};

use crate::json_value::{self, arr, obj, str as jstr};
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
                    delta: obj([("stop_reason", jstr(sr))]),
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

/// Beta that enables on-demand compaction: the `compaction` request field
/// and the `compaction` content block. Required on the compaction request
/// and on every later request that carries the block.
pub(crate) const COMPACTION_BETA: &str = "compact-2026-09-04";

/// `native`: send a compaction block as the API returned it; otherwise as
/// plain text, for a model without on-demand compaction.
fn content_block_to_anthropic_role(b: &ContentBlock, untrusted: bool, native: bool) -> Value {
    match b {
        ContentBlock::Compaction { raw, .. } if native => raw.clone(),
        ContentBlock::Compaction { content, .. } => obj([
            ("type", jstr("text")),
            ("text", jstr(ContentBlock::compaction_text(content))),
        ]),
        ContentBlock::Text { text } => {
            let text = if untrusted {
                whycodes_core::harmony::escape_replay(text)
            } else {
                text.to_string()
            };
            obj([("type", jstr("text")), ("text", jstr(&text))])
        }
        ContentBlock::Image { source } => match source {
            whycodes_core::types::ImageSource::Base64 { media_type, data } => obj([
                ("type", jstr("image")),
                (
                    "source",
                    obj([
                        ("type", jstr("base64")),
                        ("media_type", jstr(media_type)),
                        ("data", jstr(data)),
                    ]),
                ),
            ]),
            _ => obj([("type", jstr("text")), ("text", jstr("[image]"))]),
        },
        ContentBlock::ToolUse { id, name, input } => obj([
            ("type", jstr("tool_use")),
            ("id", jstr(id)),
            ("name", jstr(name)),
            ("input", input.clone()),
        ]),
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => obj([
            ("type", jstr("tool_result")),
            ("tool_use_id", jstr(tool_use_id)),
            (
                "content",
                jstr(whycodes_core::harmony::escape_replay(content)),
            ),
            ("is_error", Value::Bool(is_error.unwrap_or(false))),
        ]),
        ContentBlock::Thinking { text, signature } => {
            let mut v = obj([("type", jstr("thinking")), ("thinking", jstr(text))]);
            if let Some(sig) = signature.as_ref().filter(|s| !s.is_empty()) {
                json_value::insert(&mut v, "signature", jstr(sig));
            }
            v
        }
        ContentBlock::RedactedThinking { data } => {
            obj([("type", jstr("redacted_thinking")), ("data", jstr(data))])
        }
    }
}

pub struct AnthropicProvider {
    name: String,
    messages_url: String,
    /// `extra_body` from config, merged into every request body.
    extra_body: std::collections::HashMap<String, Value>,
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
///
/// `betas` are request features (e.g. [`COMPACTION_BETA`]); they join the
/// OAuth flag in one comma-separated `anthropic-beta` header.
fn authed_post(url: &str, api_key: &str, betas: &[&str]) -> reqwest::RequestBuilder {
    let req = crate::client_identity::post(url).header("anthropic-version", "2023-06-01");
    let key = api_key.trim();
    let oauth = key.starts_with("sk-ant-oat");
    let mut beta: Vec<&str> = Vec::with_capacity(betas.len() + 1);
    if oauth {
        beta.push("oauth-2025-04-20");
    }
    beta.extend_from_slice(betas);
    let req = if beta.is_empty() {
        req
    } else {
        req.header("anthropic-beta", beta.join(","))
    };
    if key.is_empty() {
        req
    } else if oauth {
        req.header("Authorization", format!("Bearer {key}"))
    } else {
        req.header("x-api-key", key)
    }
}

/// True when `messages` opens with a compaction block and `model` can
/// read it natively: the block is sent verbatim, with [`COMPACTION_BETA`].
fn replays_compaction(messages: &[Message], model: &str) -> bool {
    let opens_with_block = match messages.first().map(|m| &m.content) {
        Some(whycodes_core::types::MessageContent::Blocks(blocks)) => {
            matches!(blocks.first(), Some(ContentBlock::Compaction { .. }))
        }
        _ => false,
    };
    opens_with_block && crate::capabilities::detect_capabilities("anthropic", model).compaction
}

/// Usage of a response. Under the compaction beta the real cost of a
/// summarization call is in `usage.iterations` (top-level tokens are 0), so
/// the iterations are summed when present.
fn usage_from_json(usage: &Value) -> Usage {
    let (input_tokens, output_tokens) = match usage["iterations"].as_array() {
        Some(iterations) if !iterations.is_empty() => iterations.iter().fold((0, 0), |acc, it| {
            (
                acc.0 + it["input_tokens"].as_u64().unwrap_or(0),
                acc.1 + it["output_tokens"].as_u64().unwrap_or(0),
            )
        }),
        _ => (
            usage["input_tokens"].as_u64().unwrap_or(0),
            usage["output_tokens"].as_u64().unwrap_or(0),
        ),
    };
    Usage {
        input_tokens,
        output_tokens,
        cache_creation_input_tokens: usage["cache_creation_input_tokens"].as_u64(),
        cache_read_input_tokens: usage["cache_read_input_tokens"].as_u64(),
    }
}

fn block_from_json(b: &Value) -> ContentBlock {
    match b["type"].as_str().unwrap_or("text") {
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
        "compaction" => ContentBlock::compaction_from_raw(b.clone()).unwrap_or(unknown_block()),
        _ => unknown_block(),
    }
}

fn unknown_block() -> ContentBlock {
    ContentBlock::Text {
        text: "[unknown block]".to_string(),
    }
}

/// A non-streaming Messages response (`complete` and `compact`).
async fn read_response(resp: reqwest::Response, model: &str) -> whycodes_core::Result<LlmResponse> {
    let status = resp.status();
    let json: Value = resp.json().await.map_err(json_parse_error)?;

    if !status.is_success() {
        let err_msg = json["error"]["message"].as_str().unwrap_or("Unknown error");
        return Err(whycodes_core::Error::llm(format!(
            "Anthropic API error ({}): {}",
            status, err_msg
        )));
    }

    let content = json["content"]
        .as_array()
        .map(|blocks| blocks.iter().map(block_from_json).collect())
        .unwrap_or_default();

    let usage = &json["usage"];
    crate::usage_dump::dump_raw_usage("anthropic", usage);
    Ok(LlmResponse {
        content,
        stop_reason: json["stop_reason"].as_str().map(|s| s.to_string()),
        usage: usage_from_json(usage),
        model: model.to_string(),
    })
}

impl AnthropicProvider {
    pub fn new() -> Self {
        Self::from_base(None)
    }

    pub fn from_config(config: &whycodes_core::types::ProviderConfig) -> Self {
        let mut provider =
            Self::from_base(config.base_url.as_deref().or(config.api_base.as_deref()));
        provider.extra_body = config.extra.clone();
        provider
    }

    pub fn from_base(base: Option<&str>) -> Self {
        Self {
            name: "anthropic".to_string(),
            messages_url: crate::endpoint::normalize_anthropic_messages_url(base),
            extra_body: Default::default(),
        }
    }

    /// Betas a request needs beyond the credential's own.
    fn betas_for(request: &LlmRequest, model: &str) -> &'static [&'static str] {
        if replays_compaction(&request.messages, model) {
            &[COMPACTION_BETA]
        } else {
            &[]
        }
    }

    /// The on-demand compaction request: the turn's body without `stream`
    /// and without the fields the API rejects next to `compaction`, plus
    /// `"compaction": {"type": "summarize", "instructions"?}`.
    pub fn compaction_body(&self, request: &LlmRequest, model: &str, instructions: &str) -> Value {
        let mut body = self.build_body(request, model);
        body["stream"] = Value::Bool(false);
        // `build_body` always returns an object.
        body.as_object_mut().into_iter().for_each(|map| {
            for key in ["stop_sequences", "tool_choice", "context_management"] {
                map.remove(key);
            }
        });
        let mut compaction = obj([("type", jstr("summarize"))]);
        if !instructions.trim().is_empty() {
            json_value::insert(&mut compaction, "instructions", jstr(instructions));
        }
        body["compaction"] = compaction;
        body
    }

    pub fn build_body(&self, request: &LlmRequest, model: &str) -> Value {
        let native = replays_compaction(&request.messages, model);
        let mut body = obj([
            ("model", jstr(model)),
            (
                "max_tokens",
                Value::from(request.max_tokens.unwrap_or(4096)),
            ),
            (
                "messages",
                arr(self.convert_messages(&request.messages, native)),
            ),
            ("stream", Value::Bool(true)),
        ]);

        // System as plain string first; cache policy promotes + marks.
        if !request.system.is_empty() {
            body["system"] = Value::String(request.system.clone());
        }

        if !request.tools.is_empty() {
            body["tools"] = serde_json::Value::Array(self.convert_tools(&request.tools));
        }

        crate::openai_compat::apply_sampling(&mut body, request);

        crate::thinking::ThinkingConfig::apply_anthropic(&mut body, request.thinking.as_ref());

        // Breakpoints: last tool + system + latest user message.
        if request.use_prompt_cache {
            crate::cache::apply_anthropic_cache_policy(
                &mut body,
                &crate::cache::CacheConfig::default(),
            );
        }

        crate::openai_compat::apply_extra_body(&mut body, &self.extra_body);
        body
    }

    /// `native`: compaction blocks go out verbatim. Otherwise a message that
    /// opens with one is sent as `user` text, so a history that starts with
    /// the summary still starts with a user turn.
    fn convert_messages(&self, messages: &[Message], native: bool) -> Vec<Value> {
        messages
            .iter()
            .filter_map(|m| {
                let summary_as_text = !native
                    && matches!(&m.content, whycodes_core::types::MessageContent::Blocks(b)
                        if matches!(b.first(), Some(ContentBlock::Compaction { .. })));
                let role = match m.role {
                    _ if summary_as_text => "user",
                    whycodes_core::types::Role::Assistant => "assistant",
                    whycodes_core::types::Role::User => "user",
                    whycodes_core::types::Role::System => "user", // system goes in top-level
                    whycodes_core::types::Role::Tool => "user",
                };

                let untrusted = m.role != whycodes_core::types::Role::Assistant;
                let content: Vec<Value> = match &m.content {
                    whycodes_core::types::MessageContent::Text(text) => {
                        let text = if untrusted {
                            whycodes_core::harmony::escape_replay(text)
                        } else {
                            text.clone()
                        };
                        vec![obj([("type", jstr("text")), ("text", jstr(&text))])]
                    }
                    whycodes_core::types::MessageContent::Blocks(blocks) => {
                        let wire = if m.role == whycodes_core::types::Role::Assistant {
                            whycodes_core::types::strip_trailing_thinking(blocks)
                        } else {
                            blocks.clone()
                        };
                        wire.iter()
                            .map(|b| content_block_to_anthropic_role(b, untrusted, native))
                            .collect()
                    }
                };
                if content.is_empty() {
                    return None;
                }
                Some(obj([("role", jstr(role)), ("content", arr(content))]))
            })
            .collect()
    }

    fn convert_tools(&self, tools: &[ToolDefinition]) -> Vec<Value> {
        // cache_control is applied later by `apply_anthropic_cache_policy`.
        tools
            .iter()
            .map(|t| {
                obj([
                    ("name", jstr(&t.name)),
                    ("description", jstr(&t.description)),
                    ("input_schema", t.parameters.clone()),
                ])
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

            let betas = Self::betas_for(request, model);
            let resp = crate::oauth_refresh::send_with_refresh_retry(self.name(), api_key, |key| {
                authed_post(self.default_base_url(), key, betas).json(&body)
            })
            .await?;
            read_response(resp, model).await
        })
    }

    fn compact<'a>(
        &'a self,
        request: &'a LlmRequest,
        api_key: &'a str,
        model: &'a str,
        instructions: &'a str,
    ) -> ProviderResponseFuture<'a> {
        Box::pin(async move {
            let body = self.compaction_body(request, model, instructions);
            let resp = crate::oauth_refresh::send_with_refresh_retry(self.name(), api_key, |key| {
                authed_post(self.default_base_url(), key, &[COMPACTION_BETA]).json(&body)
            })
            .await?;
            read_response(resp, model).await
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
            let betas = Self::betas_for(request, model);
            let api_key = api_key.to_string();

            let resp =
                crate::oauth_refresh::send_with_refresh_retry(self.name(), &api_key, |key| {
                    authed_post(self.default_base_url(), key, betas).json(&body)
                })
                .await?;

            if !resp.status().is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(whycodes_core::Error::llm(format!(
                    "Anthropic API error: {}",
                    text
                )));
            }

            Ok(anthropic_sse(crate::openai_compat::response_bytes(resp)))
        })
    }
}

fn json_parse_error(err: impl std::fmt::Display) -> whycodes_core::Error {
    whycodes_core::Error::llm(format!("JSON parse error: {err}"))
}

#[cfg(test)]
pub(crate) fn json_parse_error_for_tests(err: &str) -> whycodes_core::Error {
    json_parse_error(err)
}

impl Default for AnthropicProvider {
    fn default() -> Self {
        Self::new()
    }
}

struct AnthropicSse {
    bytes: crate::openai_compat::ByteStream,
    buffer: crate::line_buffer::LineBuffer,
    pending: std::collections::VecDeque<whycodes_core::Result<StreamEvent>>,
    done: bool,
}

impl AnthropicSse {
    fn push_data_line(&mut self, line: &str) {
        if line.is_empty() || !line.starts_with("data: ") {
            return;
        }
        let data = &line[6..];
        if data == "[DONE]" {
            self.pending.push_back(Ok(StreamEvent::MessageStop));
            self.done = true;
            return;
        }
        self.pending.extend(events_for_data(data));
    }
}

impl Stream for AnthropicSse {
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
                    this.buffer.push(&bytes);
                    while let Some(line) = this.buffer.next_line() {
                        this.push_data_line(&line);
                        if this.done {
                            break;
                        }
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(crate::openai_compat::stream_chunk_error(
                        "anthropic",
                        e,
                    ))));
                }
                Poll::Ready(None) => this.done = true,
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

fn anthropic_sse(bytes: crate::openai_compat::ByteStream) -> ProviderEventStream {
    Box::pin(AnthropicSse {
        bytes,
        buffer: crate::line_buffer::LineBuffer::default(),
        pending: std::collections::VecDeque::new(),
        done: false,
    })
}

#[cfg(test)]
pub(crate) fn anthropic_sse_from_bytes(
    bytes: crate::openai_compat::ByteStream,
) -> ProviderEventStream {
    anthropic_sse(bytes)
}

#[cfg(test)]
#[path = "anthropic_tests.rs"]
mod tests;
