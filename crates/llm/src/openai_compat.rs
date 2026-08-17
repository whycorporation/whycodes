//! Shared conversion helpers for OpenAI chat-completions compatible APIs.
//!
//! Strict gateways (OmniRoute, some Moonshot/Kimi proxies, Azure, etc.) reject
//! assistant messages that lack plain-string `content`, `reasoning_content`, or
//! `tool_calls`. Sending Anthropic-style content *arrays* for text-only turns
//! triggers: `Assistant messages must contain text, reasoning content, or tool_calls.`

use serde_json::Value;
use whycode_core::types::{
    ContentBlock, ImageSource, LlmRequest, Message, MessageContent, Role, ToolArgumentsFormat,
    ToolDefinition,
};

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
        messages.push(serde_json::json!({
            "role": "system",
            "content": request.system
        }));
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
            serde_json::json!({
                "role": role,
                "content": text,
            })
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
                tool_calls.push(serde_json::json!({
                    "id": id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": encode_tool_arguments(input, args_format),
                    }
                }));
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
            parts.push(serde_json::json!({"type": "text", "text": text}));
        }
        parts.extend(image_parts);
        Value::Array(parts)
    };

    let empty_string_content = matches!(&content, Value::String(s) if s.is_empty());
    let mut obj = serde_json::json!({
        "role": role_str,
        "content": content,
    });

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
        ImageSource::Base64 { media_type, data } => serde_json::json!({
            "type": "image_url",
            "image_url": {
                "url": format!("data:{media_type};base64,{data}")
            }
        }),
        ImageSource::Url { url } => serde_json::json!({
            "type": "image_url",
            "image_url": { "url": url }
        }),
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
pub fn stream_events_for_tool_call_delta(tc: &Value) -> Vec<whycode_core::types::StreamEvent> {
    use whycode_core::types::StreamEvent;

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
pub fn stream_events_for_tool_calls(tool_calls: &[Value]) -> Vec<whycode_core::types::StreamEvent> {
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
    body["stream_options"] = serde_json::json!({ "include_usage": true });
}

/// Map a non-stream `usage` object from chat.completions into our [`Usage`].
///
/// OpenAI-style `prompt_tokens_details.cached_tokens` is a **subset** of
/// `prompt_tokens`, not an additive Anthropic-style cache field. Mapping it
/// into `cache_read_input_tokens` would double-count in `Usage::total()` and
/// the context meter. We leave cache fields unset here; Anthropic fills them
/// via its own provider path.
pub fn usage_from_chat_completion(usage: &Value) -> whycode_core::types::Usage {
    super::usage_dump::dump_raw_usage("openai_compat", usage);
    whycode_core::types::Usage {
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
pub fn stream_usage_from_chunk(event: &Value) -> Option<whycode_core::types::StreamEvent> {
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
    Some(whycode_core::types::StreamEvent::Usage {
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
pub fn stream_events_for_chat_delta(delta: &Value) -> Vec<whycode_core::types::StreamEvent> {
    use whycode_core::types::StreamEvent;

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
const OPENAI_UNSUPPORTED_SCHEMA_KEYS: &[&str] = &["uniqueItems", "propertyNames"];

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
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": sanitize_schema_for_openai(&t.parameters)
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use whycode_core::types::Message;

    fn req_with(messages: Vec<Message>) -> LlmRequest {
        LlmRequest {
            system: "sys".into(),
            messages: std::sync::Arc::from(messages),
            tools: vec![],
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
    fn thinking_blocks_replay_as_reasoning_content() {
        let req = req_with(vec![Message {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![
                ContentBlock::Thinking {
                    text: "plan first".into(),
                    signature: Some("sig".into()),
                },
                ContentBlock::Text {
                    text: "done".into(),
                },
            ]),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]);
        let msgs = convert_messages(&req);
        assert_eq!(msgs[1]["content"].as_str().unwrap(), "done");
        assert_eq!(msgs[1]["reasoning_content"].as_str().unwrap(), "plan first");
    }

    #[test]
    fn thinking_only_assistant_is_kept() {
        let req = req_with(vec![Message {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::Thinking {
                text: "hidden".into(),
                signature: None,
            }]),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]);
        let msgs = convert_messages(&req);
        assert_eq!(msgs.len(), 2);
        assert!(msgs[1]["content"].is_null());
        assert_eq!(msgs[1]["reasoning_content"].as_str().unwrap(), "hidden");
    }

    #[test]
    fn content_blocks_from_chat_message_reads_reasoning() {
        let message = serde_json::json!({
            "role": "assistant",
            "reasoning_content": "think",
            "content": "hi",
            "tool_calls": [{
                "id": "c1",
                "type": "function",
                "function": { "name": "read", "arguments": "{\"path\":\"a\"}" }
            }]
        });
        let blocks = content_blocks_from_chat_message(&message);
        assert!(matches!(&blocks[0], ContentBlock::Thinking { text, .. } if text == "think"));
        assert!(matches!(&blocks[1], ContentBlock::Text { text } if text == "hi"));
        assert!(matches!(&blocks[2], ContentBlock::ToolUse { name, .. } if name == "read"));
    }

    #[test]
    fn text_blocks_become_plain_string_content() {
        let req = req_with(vec![Message {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::Text {
                text: "Hello there".into(),
            }]),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]);
        let msgs = convert_messages(&req);
        // system + assistant
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[1]["content"].as_str().unwrap(), "Hello there");
        assert!(msgs[1].get("tool_calls").is_none());
    }

    #[test]
    fn empty_assistant_blocks_are_skipped() {
        let req = req_with(vec![
            Message {
                role: Role::User,
                content: MessageContent::Text("hi".into()),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
            Message {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![]),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
        ]);
        let msgs = convert_messages(&req);
        assert_eq!(msgs.len(), 2); // system + user only
        assert_eq!(msgs[1]["role"], "user");
    }

    #[test]
    fn empty_assistant_text_is_skipped() {
        let req = req_with(vec![Message {
            role: Role::Assistant,
            content: MessageContent::Text(String::new()),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]);
        let msgs = convert_messages(&req);
        assert_eq!(msgs.len(), 1); // system only
    }

    #[test]
    fn tool_use_becomes_tool_calls() {
        let req = req_with(vec![Message {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![
                ContentBlock::Text {
                    text: "Calling…".into(),
                },
                ContentBlock::ToolUse {
                    id: "call_1".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "ls"}),
                },
            ]),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]);
        let msgs = convert_messages(&req);
        let asst = &msgs[1];
        assert_eq!(asst["content"].as_str().unwrap(), "Calling…");
        let tcs = asst["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0]["id"], "call_1");
        assert_eq!(tcs[0]["type"], "function");
        assert_eq!(tcs[0]["function"]["name"], "bash");
        // arguments must be a string
        assert!(tcs[0]["function"]["arguments"].is_string());
        assert!(
            tcs[0]["function"]["arguments"]
                .as_str()
                .unwrap()
                .contains("ls")
        );
    }

    #[test]
    fn tool_only_assistant_uses_null_content() {
        let req = req_with(vec![Message {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: "c1".into(),
                name: "read".into(),
                input: serde_json::json!({}),
            }]),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]);
        let msgs = convert_messages(&req);
        let asst = &msgs[1];
        assert!(asst["content"].is_null());
        assert_eq!(asst["tool_calls"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn tool_role_keeps_tool_call_id() {
        let req = req_with(vec![Message {
            role: Role::Tool,
            content: MessageContent::Text("ok".into()),
            tool_call_id: Some("c1".into()),
            name: None,
            created_at: None,
        }]);
        let msgs = convert_messages(&req);
        assert_eq!(msgs[1]["role"], "tool");
        assert_eq!(msgs[1]["tool_call_id"], "c1");
        assert_eq!(msgs[1]["content"], "ok");
    }

    #[test]
    fn parse_tool_arguments_from_json_string() {
        let raw = Value::String(r#"{"query":"nuxt latest"}"#.into());
        let parsed = parse_tool_arguments(&raw);
        assert_eq!(parsed["query"], "nuxt latest");
    }

    #[test]
    fn null_arguments_never_become_string_null() {
        // Regression: Value::Null.to_string() == "null" breaks strict templates
        // (e.g. Kimi K3) that require a JSON object after json.loads.
        let encoded = encode_tool_arguments(&Value::Null, ToolArgumentsFormat::JsonString);
        assert_eq!(encoded.as_str().unwrap(), "{}");
        let as_obj = encode_tool_arguments(&Value::Null, ToolArgumentsFormat::Object);
        assert!(as_obj.is_object());
        assert!(as_obj.as_object().unwrap().is_empty());
    }

    #[test]
    fn object_format_is_opt_in_via_convert_messages_with_format() {
        let req = req_with(vec![Message {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: "c1".into(),
                name: "websearch".into(),
                input: serde_json::json!({"query": "nuxt"}),
            }]),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]);
        // Default path: OpenAI JSON string
        let default_msgs = convert_messages(&req);
        assert!(default_msgs[1]["tool_calls"][0]["function"]["arguments"].is_string());

        // Explicit provider config path: bare object
        let object_msgs = convert_messages_with_format(&req, ToolArgumentsFormat::Object);
        let args = &object_msgs[1]["tool_calls"][0]["function"]["arguments"];
        assert!(args.is_object(), "expected object, got {args}");
        assert_eq!(args["query"], "nuxt");
    }

    #[test]
    fn parse_tool_arguments_empty_null() {
        assert!(
            parse_tool_arguments(&Value::Null)
                .as_object()
                .unwrap()
                .is_empty()
        );
        assert!(
            parse_tool_arguments(&Value::String(String::new()))
                .as_object()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn stream_tool_call_start_then_delta() {
        use whycode_core::types::StreamEvent;

        let start = serde_json::json!({
            "index": 0,
            "id": "call_abc",
            "type": "function",
            "function": { "name": "websearch", "arguments": "" }
        });
        let events = stream_events_for_tool_call_delta(&start);
        assert_eq!(events.len(), 1);
        match &events[0] {
            StreamEvent::ToolUse { id, name, .. } => {
                assert_eq!(id, "call_abc");
                assert_eq!(name, "websearch");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }

        let delta = serde_json::json!({
            "index": 0,
            "function": { "arguments": r#"{"query":"nuxt"}"# }
        });
        let events = stream_events_for_tool_call_delta(&delta);
        assert_eq!(events.len(), 1);
        match &events[0] {
            StreamEvent::ToolUseDelta {
                id,
                input_json_delta,
            } => {
                assert_eq!(id, "0");
                assert!(input_json_delta.contains("nuxt"));
            }
            other => panic!("expected ToolUseDelta, got {other:?}"),
        }
    }

    #[test]
    fn chat_delta_emits_thinking_from_reasoning_content() {
        use whycode_core::types::StreamEvent;

        // DeepSeek / Grok-compat reasoning stream — must not be dropped.
        let delta = serde_json::json!({
            "role": "assistant",
            "reasoning_content": "Let me check the file first.",
            "content": null
        });
        let events = stream_events_for_chat_delta(&delta);
        assert_eq!(events.len(), 1);
        match &events[0] {
            StreamEvent::ThinkingDelta { text } => {
                assert!(text.contains("check the file"), "{text}");
            }
            other => panic!("expected ThinkingDelta, got {other:?}"),
        }
    }

    #[test]
    fn chat_delta_thinking_then_text_then_tools() {
        use whycode_core::types::StreamEvent;

        let delta = serde_json::json!({
            "reasoning": "plan",
            "content": "ok",
            "tool_calls": [{
                "index": 0,
                "id": "c1",
                "type": "function",
                "function": { "name": "read", "arguments": "" }
            }]
        });
        let events = stream_events_for_chat_delta(&delta);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::ThinkingDelta { text } if text == "plan")),
            "{events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::TextDelta { text } if text == "ok")),
            "{events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::ToolUse { name, .. } if name == "read")),
            "{events:?}"
        );
    }

    #[test]
    fn nested_reasoning_object_is_parsed() {
        use whycode_core::types::StreamEvent;

        let delta = serde_json::json!({
            "reasoning": { "content": "nested thought" }
        });
        let events = stream_events_for_chat_delta(&delta);
        match events.as_slice() {
            [StreamEvent::ThinkingDelta { text }] => assert_eq!(text, "nested thought"),
            other => panic!("expected single ThinkingDelta, got {other:?}"),
        }
    }

    #[test]
    fn whitespace_only_reasoning_is_skipped() {
        let delta = serde_json::json!({
            "reasoning_content": "  \n\t  ",
            "content": "hi"
        });
        let events = stream_events_for_chat_delta(&delta);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            whycode_core::types::StreamEvent::TextDelta { text } if text == "hi"
        ));
    }

    #[test]
    fn empty_delta_emits_nothing() {
        let delta = serde_json::json!({"role": "assistant"});
        assert!(stream_events_for_chat_delta(&delta).is_empty());
    }

    // ── Regressions reported against other agents: schema sanitize ────────

    use serde_json::json;

    #[test]
    fn sanitize_strips_unsupported_keywords_recursively() {
        // jcode#687 (uniqueItems) + jcode#754 (propertyNames): a single
        // unsupported keyword used to 400 the whole tool catalog.
        let schema = json!({
            "type": "object",
            "properties": {
                "ids": {
                    "type": "array",
                    "items": { "type": "string" },
                    "uniqueItems": true
                },
                "data": {
                    "type": "object",
                    "propertyNames": { "type": "string" },
                    "additionalProperties": { "type": "string" }
                }
            }
        });
        let out = sanitize_schema_for_openai(&schema);
        assert!(out.pointer("/properties/ids/uniqueItems").is_none());
        assert!(out.pointer("/properties/data/propertyNames").is_none());
        // Everything else is preserved.
        assert_eq!(
            out.pointer("/properties/ids/items/type"),
            Some(&json!("string"))
        );
        assert!(
            out.pointer("/properties/data/additionalProperties")
                .is_some()
        );
    }

    #[test]
    fn sanitize_adds_missing_types() {
        // jcode#713: a property without `type` used to 400 OpenAI.
        let schema = json!({
            "properties": {
                "key": { "type": "string" },
                "value": { "description": "JSON type depends on the key." }
            },
            "required": ["key", "value"]
        });
        let out = sanitize_schema_for_openai(&schema);
        // Root: properties+required → object.
        assert_eq!(out["type"], json!("object"));
        // Leaf without `type`: full union (every JSON value accepted).
        assert_eq!(
            out.pointer("/properties/value/type"),
            Some(&json!([
                "string", "number", "integer", "boolean", "object", "array", "null"
            ]))
        );
        // Node with `items` but no `type` → array.
        let arr = sanitize_schema_for_openai(&json!({ "items": { "type": "string" } }));
        assert_eq!(arr["type"], json!("array"));
        // Nodes carrying anyOf/$ref are left untouched.
        let any = sanitize_schema_for_openai(&json!({ "anyOf": [{ "type": "string" }] }));
        assert!(any.get("type").is_none());
    }

    #[test]
    fn sanitize_leaves_annotation_values_untouched() {
        // Data inside `default` is not a schema; it must not gain a type.
        let schema = json!({
            "type": "object",
            "properties": {
                "opts": {
                    "type": "object",
                    "default": { "retries": 3 },
                    "examples": [{ "retries": 1 }]
                }
            }
        });
        let out = sanitize_schema_for_openai(&schema);
        assert_eq!(
            out.pointer("/properties/opts/default"),
            Some(&json!({ "retries": 3 }))
        );
        assert_eq!(
            out.pointer("/properties/opts/examples"),
            Some(&json!([{ "retries": 1 }]))
        );
    }

    #[test]
    fn convert_tools_sanitizes_parameters() {
        let tools = vec![ToolDefinition {
            name: "mcp__x__y".into(),
            description: "d".into(),
            parameters: json!({
                "type": "object",
                "properties": { "ids": { "type": "array", "uniqueItems": true } }
            }),
        }];
        let out = convert_tools(&tools);
        assert!(
            out[0]
                .pointer("/function/parameters/properties/ids/uniqueItems")
                .is_none()
        );
    }

    #[test]
    fn truncated_tool_arguments_yield_empty_object() {
        // opencode#36766: a truncated stream yields a harmless {} instead of a panic/400.
        let parsed = parse_tool_arguments(&json!(r#"{"patchText": "@@ -1,3"#));
        assert_eq!(parsed, json!({}));
        assert_eq!(parse_tool_arguments(&json!("")), json!({}));
        assert_eq!(parse_tool_arguments(&Value::Null), json!({}));
    }

    #[test]
    fn usage_from_chat_completion_ignores_openai_cached_subset() {
        // cached_tokens is inside prompt_tokens — must not become additive cache_read.
        let usage = usage_from_chat_completion(&json!({
            "prompt_tokens": 1200,
            "completion_tokens": 40,
            "prompt_tokens_details": { "cached_tokens": 900 },
        }));
        assert_eq!(usage.input_tokens, 1200);
        assert_eq!(usage.output_tokens, 40);
        assert_eq!(usage.cache_read_input_tokens, None);
        assert_eq!(usage.cache_creation_input_tokens, None);
        assert_eq!(usage.total(), 1240);
    }

    #[test]
    fn stream_usage_from_final_include_usage_chunk() {
        // OpenAI final chunk: empty choices, usage only (no finish_reason).
        let event = json!({
            "choices": [],
            "usage": { "prompt_tokens": 1500, "completion_tokens": 12 }
        });
        match stream_usage_from_chunk(&event) {
            Some(whycode_core::types::StreamEvent::Usage {
                input_tokens,
                output_tokens,
            }) => {
                assert_eq!(input_tokens, 1500);
                assert_eq!(output_tokens, 12);
            }
            other => panic!("expected Usage event, got {other:?}"),
        }
        assert!(stream_usage_from_chunk(&json!({ "choices": [] })).is_none());
        assert!(
            stream_usage_from_chunk(&json!({
                "usage": { "prompt_tokens": 0, "completion_tokens": 0 }
            }))
            .is_none()
        );
    }

    #[test]
    fn attach_stream_usage_option_sets_include_usage() {
        let mut body = json!({ "model": "x", "stream": true });
        attach_stream_usage_option(&mut body);
        assert_eq!(body["stream_options"]["include_usage"], true);
    }
}
