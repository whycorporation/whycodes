//! Shared conversion helpers for OpenAI chat-completions compatible APIs.
//!
//! Strict gateways (OmniRoute, some Moonshot/Kimi proxies, Azure, etc.) reject
//! assistant messages that lack plain-string `content`, `reasoning_content`, or
//! `tool_calls`. Sending Anthropic-style content *arrays* for text-only turns
//! triggers: `Assistant messages must contain text, reasoning content, or tool_calls.`

use std::pin::Pin;
use std::task::{Context, Poll};

use futures::Stream;
use futures::StreamExt;
use serde_json::Value;
use whycodes_core::types::{
    ContentBlock, ImageSource, LlmRequest, Message, MessageContent, Role, StreamEvent,
    ToolArgumentsFormat, ToolDefinition,
};

use crate::json_value::{self, arr, obj, str as jstr};
use crate::provider::ProviderEventStream;

/// JSON number from a finite float. `None` for NaN/Inf — JSON has no non-finite numbers,
/// and `Number::from_f64` would otherwise force an `unwrap` (abort in release).
pub fn json_number(n: impl Into<f64>) -> Option<Value> {
    serde_json::Number::from_f64(n.into()).map(Value::Number)
}

/// Set `body[key]` when `n` is finite; skip NaN/Inf so encoding never panics.
pub fn set_json_f64(body: &mut Value, key: &str, n: impl Into<f64>) {
    if let Some(v) = json_number(n) {
        body[key] = v;
    }
}

/// Copy finite `temperature` / `top_p` onto an OpenAI-style request body.
pub fn apply_sampling(body: &mut Value, request: &LlmRequest) {
    if let Some(temp) = request.temperature {
        set_json_f64(body, "temperature", temp);
    }
    if let Some(top_p) = request.top_p {
        set_json_f64(body, "top_p", top_p);
    }
}

/// Shared chat-completions request body used by OpenAI-compatible providers.
pub fn chat_completions_body(request: &LlmRequest, model: &str, messages: Vec<Value>) -> Value {
    let mut body = obj([
        ("model", jstr(model)),
        ("messages", arr(messages)),
        ("stream", Value::Bool(true)),
    ]);
    if let Some(max_tokens) = request.max_tokens {
        json_value::insert(&mut body, "max_tokens", Value::from(max_tokens));
    }
    if !request.tools.is_empty() {
        json_value::insert(&mut body, "tools", arr(convert_tools(&request.tools)));
        json_value::insert(&mut body, "tool_choice", jstr("auto"));
        json_value::insert(&mut body, "parallel_tool_calls", Value::Bool(true));
    }
    apply_sampling(&mut body, request);
    crate::thinking::ThinkingConfig::apply_openai_effort(&mut body, request.thinking.as_ref());
    body
}

/// Flatten `err` + `source()` chain so reqwest decode failures keep TLS/EOF/JSON
/// causes that `{e}` alone drops (`error decoding response body`).
pub fn error_source_chain(err: &dyn std::error::Error) -> String {
    let mut out = err.to_string();
    let mut cur = err.source();
    let mut n = 0;
    while let Some(src) = cur {
        n += 1;
        if n > 8 {
            break;
        }
        let s = src.to_string();
        if !s.is_empty() && !out.contains(&s) {
            out = format!("{out}: {s}");
        }
        cur = src.source();
    }
    out
}

/// Log a mid-stream chunk failure to `unified.jsonl` and return the LLM error.
///
/// TUI already records `turn.error` with the display string; this captures the
/// provider + reqwest source chain at the decode site (always-on JSONL).
pub fn stream_chunk_error(provider: &str, err: impl std::fmt::Display) -> whycodes_core::Error {
    let chain = err.to_string();
    whycodes_core::logging::emit(
        "llm",
        "error",
        "llm.stream_chunk",
        Some(obj([("provider", jstr(provider)), ("error", jstr(&chain))])),
    );
    tracing::warn!("llm.stream_chunk provider={provider} error={chain}");
    whycodes_core::Error::llm(format!("Stream: {chain}"))
}

/// SSE parser for OpenAI chat-completions streams.
///
/// A hand-written [`Stream`] instead of `async_stream::stream!` so llvm-cov
/// counts the executed lines instead of the macro expansion.
pub(crate) type ByteStream = Pin<Box<dyn Stream<Item = Result<Vec<u8>, String>> + Send>>;

pub(crate) fn response_bytes(resp: reqwest::Response) -> ByteStream {
    Box::pin(resp.bytes_stream().map(|chunk| {
        chunk
            .map(|b| b.to_vec())
            .map_err(|e| error_source_chain(&e))
    }))
}

pub fn chat_sse_stream(resp: reqwest::Response, provider: &str) -> ProviderEventStream {
    chat_sse_from_bytes(response_bytes(resp), provider)
}

pub(crate) fn chat_sse_from_bytes(bytes: ByteStream, provider: &str) -> ProviderEventStream {
    Box::pin(ChatSse {
        bytes,
        buffer: String::new(),
        pending: std::collections::VecDeque::new(),
        provider: provider.to_string(),
        done: false,
    })
}

struct ChatSse {
    bytes: ByteStream,
    buffer: String,
    pending: std::collections::VecDeque<whycodes_core::Result<StreamEvent>>,
    provider: String,
    done: bool,
}

impl ChatSse {
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
        let Ok(event) = serde_json::from_str::<Value>(data) else {
            return;
        };
        let delta = &event["choices"][0]["delta"];
        for ev in stream_events_for_chat_delta(delta) {
            self.pending.push_back(Ok(ev));
        }
        if let Some(ev) = stream_usage_from_chunk(&event) {
            self.pending.push_back(Ok(ev));
        }
    }

    fn drain_buffer_lines(&mut self) {
        while let Some(pos) = self.buffer.find('\n') {
            let line = self.buffer[..pos].trim().to_string();
            self.buffer = self.buffer[pos + 1..].to_string();
            self.push_data_line(&line);
            if self.done {
                break;
            }
        }
    }
}

impl Stream for ChatSse {
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
                    this.drain_buffer_lines();
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(stream_chunk_error(&this.provider, e))));
                }
                Poll::Ready(None) => this.done = true,
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// Convert request messages into OpenAI chat-completions format.
///
/// Uses OpenAI-style JSON-string tool arguments. For a different wire shape,
/// call [`convert_messages_with_format`] with the provider's configured format.
pub fn convert_messages(request: &LlmRequest) -> Vec<Value> {
    convert_messages_with_format(request, ToolArgumentsFormat::JsonString)
}

/// Convert request messages with an explicit tool-arguments wire format.
///
/// The format comes from **provider config** (`ProviderConfig::tool_arguments`),
/// not from sniffing model ids.
pub fn convert_messages_with_format(
    request: &LlmRequest,
    args_format: ToolArgumentsFormat,
) -> Vec<Value> {
    let mut messages: Vec<Value> = Vec::new();

    if !request.system.is_empty() {
        messages.push(obj([
            ("role", jstr("system")),
            ("content", jstr(&request.system)),
        ]));
    }

    for msg in request.messages.iter() {
        if let Some(obj) = convert_one_message(msg, args_format) {
            messages.push(obj);
        }
    }

    messages
}

fn convert_one_message(msg: &Message, args_format: ToolArgumentsFormat) -> Option<Value> {
    let role = match msg.role {
        Role::Assistant => "assistant",
        Role::User => "user",
        Role::System => "system",
        Role::Tool => "tool",
    };

    let mut obj = match &msg.content {
        MessageContent::Text(text) => {
            // Empty assistant text with no tool_calls is invalid for strict APIs.
            if msg.role == Role::Assistant && text.is_empty() {
                return None;
            }
            obj([("role", jstr(role)), ("content", jstr(text))])
        }
        MessageContent::Blocks(blocks) => {
            convert_blocks_message(role, &msg.role, blocks, args_format)?
        }
    };

    if let Some(tool_call_id) = &msg.tool_call_id {
        obj["tool_call_id"] = Value::String(tool_call_id.clone());
    }
    if let Some(name) = &msg.name {
        obj["name"] = Value::String(name.clone());
    }

    Some(obj)
}

fn convert_blocks_message(
    role_str: &str,
    role: &Role,
    blocks: &[ContentBlock],
    args_format: ToolArgumentsFormat,
) -> Option<Value> {
    let mut text_parts: Vec<String> = Vec::new();
    let mut image_parts: Vec<Value> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut thinking_parts: Vec<String> = Vec::new();
    // ToolResult inside blocks is rare (usually a Tool-role message); keep as
    // text so the model still sees the payload rather than dropping it.
    let mut extra_text: Vec<String> = Vec::new();

    for block in blocks {
        match block {
            ContentBlock::Text { text } => {
                if !text.is_empty() {
                    text_parts.push(text.clone());
                }
            }
            ContentBlock::Image { source } => {
                image_parts.push(image_part(source));
            }
            ContentBlock::ToolUse { id, name, input } => {
                tool_calls.push(obj([
                    ("id", jstr(id)),
                    ("type", jstr("function")),
                    (
                        "function",
                        obj([
                            ("name", jstr(name)),
                            ("arguments", encode_tool_arguments(input, args_format)),
                        ]),
                    ),
                ]));
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => {
                extra_text.push(format!("[tool_result {tool_use_id}] {content}"));
            }
            // Replay as `reasoning_content` (DeepSeek / Grok). Never dump
            // thoughts into visible `content`.
            ContentBlock::Thinking { text, .. } => {
                if !text.is_empty() {
                    thinking_parts.push(text.clone());
                }
            }
            ContentBlock::RedactedThinking { .. } => {}
        }
    }

    text_parts.extend(extra_text);
    let text = text_parts.join("");

    // Strict APIs: assistant must have non-empty text and/or tool_calls.
    if *role == Role::Assistant
        && text.is_empty()
        && tool_calls.is_empty()
        && image_parts.is_empty()
        && thinking_parts.is_empty()
    {
        return None;
    }

    let content = if image_parts.is_empty() {
        // Plain string — required by OmniRoute / Moonshot-style validators.
        Value::String(text)
    } else {
        let mut parts: Vec<Value> = Vec::new();
        if !text.is_empty() {
            parts.push(obj([("type", jstr("text")), ("text", jstr(text))]));
        }
        parts.extend(image_parts);
        Value::Array(parts)
    };

    let empty_string_content = matches!(&content, Value::String(s) if s.is_empty());
    let mut obj = obj([("role", jstr(role_str)), ("content", content)]);

    if !tool_calls.is_empty() {
        obj["tool_calls"] = Value::Array(tool_calls);
        // OpenAI allows null content when the turn is tool-calls only.
        if empty_string_content {
            obj["content"] = Value::Null;
        }
    }

    if !thinking_parts.is_empty() {
        obj["reasoning_content"] = Value::String(thinking_parts.join(""));
        if empty_string_content {
            obj["content"] = Value::Null;
        }
    }

    Some(obj)
}

fn image_part(source: &ImageSource) -> Value {
    match source {
        ImageSource::Base64 { media_type, data } => obj([
            ("type", jstr("image_url")),
            (
                "image_url",
                obj([("url", jstr(format!("data:{media_type};base64,{data}")))]),
            ),
        ]),
        ImageSource::Url { url } => obj([
            ("type", jstr("image_url")),
            ("image_url", obj([("url", jstr(url))])),
        ]),
    }
}

/// Coerce tool input into a JSON **object**.
///
/// Never yields `null` / arrays / scalars: Kimi K3's chat template rejects any
/// non-object after `json.loads` (including the string `"null"`).
pub fn ensure_object_arguments(input: &Value) -> Value {
    match input {
        Value::Object(_) => input.clone(),
        Value::Null => Value::Object(Default::default()),
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty() || s == "null" {
                return Value::Object(Default::default());
            }
            match serde_json::from_str::<Value>(s) {
                Ok(Value::Object(map)) => Value::Object(map),
                // Non-object JSON (array/scalar) or garbage → empty object.
                _ => Value::Object(Default::default()),
            }
        }
        // Arrays / scalars are not valid tool argument maps.
        _ => Value::Object(Default::default()),
    }
}

/// Encode tool arguments for the wire format expected by the target API.
pub fn encode_tool_arguments(input: &Value, format: ToolArgumentsFormat) -> Value {
    let obj = ensure_object_arguments(input);
    match format {
        ToolArgumentsFormat::JsonString => Value::String(obj.to_string()),
        ToolArgumentsFormat::Object => obj,
    }
}

/// Pull thinking + text + tool_use blocks from a chat-completions `message`.
pub fn content_blocks_from_chat_message(message: &Value) -> Vec<ContentBlock> {
    let mut content = Vec::new();
    for key in ["reasoning_content", "reasoning", "thinking"] {
        let text = match message.get(key) {
            Some(Value::String(s)) => s.as_str(),
            Some(Value::Object(map)) => map.get("content").and_then(|v| v.as_str()).unwrap_or(""),
            _ => "",
        };
        if !text.trim().is_empty() {
            content.push(ContentBlock::Thinking {
                text: text.to_string(),
                signature: None,
            });
            break;
        }
    }
    if let Some(text) = message
        .get("content")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        content.push(ContentBlock::Text {
            text: text.to_string(),
        });
    }
    if let Some(tool_calls) = message.get("tool_calls").and_then(|v| v.as_array()) {
        for tc in tool_calls {
            let func = &tc["function"];
            content.push(ContentBlock::ToolUse {
                id: tc["id"].as_str().unwrap_or("").to_string(),
                name: func["name"].as_str().unwrap_or("").to_string(),
                input: parse_tool_arguments(&func["arguments"]),
            });
        }
    }
    content
}

/// Parse OpenAI-style `function.arguments` into a JSON object.
///
/// Providers send arguments as a JSON *string* (`"{\"query\":\"x\"}"`). Streaming
/// first chunks may be null/empty; complete responses should always parse.
pub fn parse_tool_arguments(raw: &Value) -> Value {
    ensure_object_arguments(raw)
}

/// String fragment from a streaming `function.arguments` field (for ToolUseDelta).
pub fn arguments_stream_fragment(raw: &Value) -> Option<String> {
    match raw {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Object(map) if !map.is_empty() => Some(raw.to_string()),
        _ => None,
    }
}

/// Convert one OpenAI streaming `delta.tool_calls[i]` entry into stream events.
///
/// OpenAI sends the call id/name on the first chunk (often with empty arguments),
/// then subsequent chunks only have `index` + argument fragments. We emit:
/// - `ToolUse` when `id` is present
/// - `ToolUseDelta` for any non-empty arguments fragment, keyed by real id when
///   known, otherwise by `index` (agent maps index → tool)
pub fn stream_events_for_tool_call_delta(tc: &Value) -> Vec<whycodes_core::types::StreamEvent> {
    use whycodes_core::types::StreamEvent;

    let mut out = Vec::new();
    let index_key = tc
        .get("index")
        .map(|i| match i {
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.clone(),
            _ => "0".into(),
        })
        .unwrap_or_else(|| "0".into());

    let id = tc["id"].as_str().filter(|s| !s.is_empty());
    let name = tc
        .pointer("/function/name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let args = tc
        .pointer("/function/arguments")
        .cloned()
        .unwrap_or(Value::Null);

    if let Some(id) = id {
        // Keep raw args on ToolUse so the agent can seed its buffer when the
        // first chunk already carries a fragment (or a full JSON string).
        // Do *not* also emit ToolUseDelta for the same fragment — that would
        // double-count.
        out.push(StreamEvent::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input: args,
        });
    } else if let Some(frag) = arguments_stream_fragment(&args) {
        out.push(StreamEvent::ToolUseDelta {
            id: index_key,
            input_json_delta: frag,
        });
    }

    out
}

/// Emit stream events for every entry in `delta.tool_calls`.
pub fn stream_events_for_tool_calls(
    tool_calls: &[Value],
) -> Vec<whycodes_core::types::StreamEvent> {
    tool_calls
        .iter()
        .flat_map(stream_events_for_tool_call_delta)
        .collect()
}

/// Ask OpenAI-compatible gateways for a final usage chunk on streaming calls.
///
/// Without `stream_options.include_usage`, many providers never report token
/// counts on the stream — the TUI context meter then stays at 0 or an old
/// estimate. Safe to attach whenever `"stream": true` is set; non-stream
/// complete() paths should strip or overwrite `stream` to false (they already
/// ignore unknown stream_options when stream is false, or never send it).
pub fn attach_stream_usage_option(body: &mut Value) {
    json_value::insert(
        body,
        "stream_options",
        obj([("include_usage", Value::Bool(true))]),
    );
}

/// Map a non-stream `usage` object from chat.completions into our [`Usage`].
///
/// OpenAI-style `prompt_tokens_details.cached_tokens` is a **subset** of
/// `prompt_tokens`, not an additive Anthropic-style cache field. Mapping it
/// into `cache_read_input_tokens` would double-count in `Usage::total()` and
/// the context meter. We leave cache fields unset here; Anthropic fills them
/// via its own provider path.
pub fn usage_from_chat_completion(usage: &Value) -> whycodes_core::types::Usage {
    super::usage_dump::dump_raw_usage("openai_compat", usage);
    whycodes_core::types::Usage {
        input_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0),
        output_tokens: usage["completion_tokens"].as_u64().unwrap_or(0),
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    }
}

/// Parse `StreamEvent::Usage` from any chat.completion.chunk that carries usage.
///
/// OpenAI's `include_usage` final chunk often has empty `choices` and no
/// `finish_reason` — only a `usage` object. Older parsers that required
/// `finish_reason` dropped that chunk and the meter never updated.
pub fn stream_usage_from_chunk(event: &Value) -> Option<whycodes_core::types::StreamEvent> {
    let usage = event.get("usage")?;
    if !usage.is_object() {
        return None;
    }
    super::usage_dump::dump_raw_usage("openai_compat", usage);
    let input_tokens = usage
        .get("prompt_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output_tokens = usage
        .get("completion_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if input_tokens == 0 && output_tokens == 0 {
        return None;
    }
    Some(whycodes_core::types::StreamEvent::Usage {
        input_tokens,
        output_tokens,
    })
}

/// Convert one OpenAI-compatible streaming `choices[0].delta` into events.
///
/// Handles the fields used by Grok / DeepSeek / OpenRouter reasoning models:
/// - `reasoning_content` / `reasoning` / `thinking` → [`StreamEvent::ThinkingDelta`]
/// - `content` → [`StreamEvent::TextDelta`]
/// - `tool_calls` → tool start/delta events
///
/// Without this, reasoning streams are dropped and the TUI never paints
/// "Thinking…" even though the model is thinking (Grok parity).
pub fn stream_events_for_chat_delta(delta: &Value) -> Vec<whycodes_core::types::StreamEvent> {
    use whycodes_core::types::StreamEvent;

    // Hot path: one SSE line per token. Cap small to avoid realloc.
    let mut out = Vec::with_capacity(3);

    // Reasoning / extended thinking (order: most common first).
    if let Some(text) = reasoning_text_from_delta(delta) {
        out.push(StreamEvent::ThinkingDelta { text });
    }

    if let Some(text) = delta.get("content").and_then(|v| v.as_str())
        && !text.is_empty()
    {
        out.push(StreamEvent::TextDelta {
            text: text.to_string(),
        });
    }

    if let Some(tool_calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
        out.reserve(tool_calls.len());
        out.extend(stream_events_for_tool_calls(tool_calls));
    }

    out
}

/// Pull reasoning text from a chat-completions delta or message object.
///
/// Providers disagree on the field name:
/// - DeepSeek / many OpenAI-compat: `reasoning_content`
/// - Some Grok / OpenRouter: `reasoning` (string) or `reasoning.content`
/// - A few: `thinking`
///
/// Empty / whitespace-only values are ignored so we never emit a no-op
/// `ThinkingDelta` that would dirty the TUI for nothing.
pub fn reasoning_text_from_delta(delta: &Value) -> Option<String> {
    for key in ["reasoning_content", "reasoning", "thinking"] {
        match delta.get(key) {
            Some(Value::String(s)) if !s.is_empty() && !s.chars().all(char::is_whitespace) => {
                return Some(s.clone());
            }
            Some(Value::Object(map)) => {
                // Nested: `{ "content": "…" }` or `{ "text": "…" }`
                for nested in ["content", "text"] {
                    if let Some(s) = map.get(nested).and_then(|v| v.as_str())
                        && !s.is_empty()
                        && !s.chars().all(char::is_whitespace)
                    {
                        return Some(s.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// JSON Schema keywords that strict OpenAI-compatible endpoints reject
/// (regression: jcode#687 `uniqueItems`, jcode#754 `propertyNames`). One bad
/// tool fails the *whole* request there, so these are stripped recursively.
const OPENAI_UNSUPPORTED_SCHEMA_KEYS: &[&str] = &[
    "uniqueItems",
    "propertyNames",
    // Code Assist proto-JSON rejects `$schema` / `$id` on tool parameters
    // (Claude-via-Antigravity 400: Unknown name "$schema").
    "$schema",
    "$id",
];

/// The full JSON type union, used when a schema omits `type` entirely: legal
/// JSON Schema (accepts anything) but rejected by OpenAI's narrower subset
/// (regression: jcode#713 — an MCP property with only a `description` 400'd
/// the entire tool catalog).
const ANY_JSON_TYPE: [&str; 7] = [
    "string", "number", "integer", "boolean", "object", "array", "null",
];

/// Rewrite a JSON Schema into the subset strict OpenAI-compatible endpoints
/// accept:
///
/// - unsupported keywords are stripped, recursively
/// - schema nodes without a type discriminator gain one: `object` when they
///   carry `properties`/`required`, `array` for `items`, otherwise the full
///   union (a bare `{}` accepts any JSON value)
///
/// Only schema positions are rewritten. Annotation values — `default`,
/// `examples`, `enum`, `const` — pass through untouched, so
/// `{"default": {"x": 1}}` does not grow a spurious `"type": "object"`.
pub fn sanitize_schema_for_openai(schema: &Value) -> Value {
    let Value::Object(map) = schema else {
        return schema.clone();
    };
    let mut out = serde_json::Map::with_capacity(map.len() + 1);
    for (k, v) in map {
        if OPENAI_UNSUPPORTED_SCHEMA_KEYS.contains(&k.as_str()) {
            continue;
        }
        let rewritten = match k.as_str() {
            // Map-of-schemas positions.
            "properties" | "$defs" | "definitions" | "patternProperties" | "dependentSchemas" => {
                match v {
                    Value::Object(props) => Value::Object(
                        props
                            .iter()
                            .map(|(name, sub)| (name.clone(), sanitize_schema_for_openai(sub)))
                            .collect(),
                    ),
                    other => other.clone(),
                }
            }
            // Single-schema positions.
            "items"
            | "additionalProperties"
            | "contains"
            | "not"
            | "if"
            | "then"
            | "else"
            | "unevaluatedItems"
            | "additionalItems" => sanitize_schema_for_openai(v),
            // Array-of-schemas positions.
            "anyOf" | "oneOf" | "allOf" | "prefixItems" => match v {
                Value::Array(arr) => {
                    Value::Array(arr.iter().map(sanitize_schema_for_openai).collect())
                }
                other => other.clone(),
            },
            // Everything else (type, description, default, examples, enum,
            // const, required, minimum, …) is data, not schema: keep as-is.
            _ => v.clone(),
        };
        out.insert(k.clone(), rewritten);
    }

    let has_discriminator = ["type", "anyOf", "oneOf", "allOf", "$ref", "not"]
        .iter()
        .any(|k| out.contains_key(*k));
    if !has_discriminator {
        let ty = if out.contains_key("properties") || out.contains_key("required") {
            Value::String("object".into())
        } else if out.contains_key("items") || out.contains_key("prefixItems") {
            Value::String("array".into())
        } else {
            Value::Array(
                ANY_JSON_TYPE
                    .iter()
                    .map(|t| Value::String((*t).into()))
                    .collect(),
            )
        };
        out.insert("type".into(), ty);
    }
    Value::Object(out)
}

/// Convert tool definitions to OpenAI `tools` array entries.
pub fn convert_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            obj([
                ("type", jstr("function")),
                (
                    "function",
                    obj([
                        ("name", jstr(&t.name)),
                        ("description", jstr(&t.description)),
                        ("parameters", sanitize_schema_for_openai(&t.parameters)),
                    ]),
                ),
            ])
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn scripted_bytes(
    chunks: impl IntoIterator<Item = Result<Vec<u8>, String>>,
) -> ByteStream {
    let chunks: Vec<_> = chunks.into_iter().collect();
    Box::pin(futures::stream::iter(chunks))
}

#[cfg(test)]
#[path = "openai_compat_tests.rs"]
mod tests;
