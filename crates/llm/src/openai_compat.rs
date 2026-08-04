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

    for msg in &request.messages {
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
        }
    }

    text_parts.extend(extra_text);
    let text = text_parts.join("");

    // Strict APIs: assistant must have non-empty text and/or tool_calls.
    if *role == Role::Assistant && text.is_empty() && tool_calls.is_empty() && image_parts.is_empty()
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

    let mut obj = serde_json::json!({
        "role": role_str,
        "content": content,
    });

    if !tool_calls.is_empty() {
        obj["tool_calls"] = Value::Array(tool_calls);
        // OpenAI allows null content when the turn is tool-calls only.
        if matches!(content, Value::String(ref s) if s.is_empty()) {
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
                    "parameters": t.parameters
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
            messages,
            tools: vec![],
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            thinking: None,
        }
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
            },
            Message {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![]),
                tool_call_id: None,
                name: None,
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
        }]);
        // Default path: OpenAI JSON string
        let default_msgs = convert_messages(&req);
        assert!(default_msgs[1]["tool_calls"][0]["function"]["arguments"].is_string());

        // Explicit provider config path: bare object
        let object_msgs =
            convert_messages_with_format(&req, ToolArgumentsFormat::Object);
        let args = &object_msgs[1]["tool_calls"][0]["function"]["arguments"];
        assert!(args.is_object(), "expected object, got {args}");
        assert_eq!(args["query"], "nuxt");
    }

    #[test]
    fn parse_tool_arguments_empty_null() {
        assert!(parse_tool_arguments(&Value::Null).as_object().unwrap().is_empty());
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
}
