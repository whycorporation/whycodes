use super::*;
use whycodes_core::types::Message;

fn req_with(messages: Vec<Message>) -> LlmRequest {
    LlmRequest {
        system: "sys".into(),
        messages: std::sync::Arc::from(messages),
        tools: std::sync::Arc::from([]),
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
fn empty_system_is_omitted_from_converted_messages() {
    let mut req = req_with(vec![Message {
        role: Role::User,
        content: MessageContent::Text("hi".into()),
        tool_call_id: None,
        name: None,
        created_at: None,
    }]);
    req.system.clear();
    let msgs = convert_messages(&req);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["role"], "user");
}

#[test]
fn empty_text_and_thinking_blocks_are_dropped() {
    let req = req_with(vec![Message {
        role: Role::Assistant,
        content: MessageContent::Blocks(vec![
            ContentBlock::Text {
                text: String::new(),
            },
            ContentBlock::Thinking {
                text: String::new(),
                signature: None,
            },
            ContentBlock::Text {
                text: "kept".into(),
            },
        ]),
        tool_call_id: None,
        name: None,
        created_at: None,
    }]);
    let msgs = convert_messages(&req);
    assert_eq!(msgs[1]["content"].as_str().unwrap(), "kept");
    assert!(msgs[1].get("reasoning_content").is_none());
}

#[test]
fn tool_call_delta_without_index_uses_zero() {
    use whycodes_core::types::StreamEvent;
    let events = stream_events_for_tool_call_delta(&json!({
        "function": { "arguments": "{\"q\":1}" }
    }));
    assert!(matches!(
        &events[0],
        StreamEvent::ToolUseDelta { id, .. } if id == "0"
    ));
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
    use whycodes_core::types::StreamEvent;

    let start = serde_json::json!({
        "index": 0,
        "id": "call_abc",
        "type": "function",
        "function": { "name": "websearch", "arguments": "" }
    });
    let events = stream_events_for_tool_call_delta(&start);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        StreamEvent::ToolUse { id, name, .. } if id == "call_abc" && name == "websearch"
    ));

    let delta = serde_json::json!({
        "index": 0,
        "function": { "arguments": r#"{"query":"nuxt"}"# }
    });
    let events = stream_events_for_tool_call_delta(&delta);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        StreamEvent::ToolUseDelta { id, input_json_delta }
            if id == "0" && input_json_delta.contains("nuxt")
    ));
}

#[test]
fn chat_delta_emits_thinking_from_reasoning_content() {
    use whycodes_core::types::StreamEvent;

    // DeepSeek / Grok-compat reasoning stream — must not be dropped.
    let delta = serde_json::json!({
        "role": "assistant",
        "reasoning_content": "Let me check the file first.",
        "content": null
    });
    let events = stream_events_for_chat_delta(&delta);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        StreamEvent::ThinkingDelta { text } if text.contains("check the file")
    ));
}

#[test]
fn chat_delta_thinking_then_text_then_tools() {
    use whycodes_core::types::StreamEvent;

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
    use whycodes_core::types::StreamEvent;

    let delta = serde_json::json!({
        "reasoning": { "content": "nested thought" }
    });
    let events = stream_events_for_chat_delta(&delta);
    assert!(matches!(
        events.as_slice(),
        [StreamEvent::ThinkingDelta { text }] if text == "nested thought"
    ));
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
        whycodes_core::types::StreamEvent::TextDelta { text } if text == "hi"
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
    let with_meta = sanitize_schema_for_openai(&json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "urn:example",
        "type": "object",
    }));
    assert!(with_meta.get("$schema").is_none());
    assert!(with_meta.get("$id").is_none());
    assert_eq!(with_meta["type"], json!("object"));
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
fn error_source_chain_walks_nested_sources() {
    #[derive(Debug)]
    struct Inner;
    impl std::fmt::Display for Inner {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("unexpected EOF during chunk")
        }
    }
    impl std::error::Error for Inner {}

    #[derive(Debug)]
    struct Outer(Inner);
    impl std::fmt::Display for Outer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("error decoding response body")
        }
    }
    impl std::error::Error for Outer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    let chain = error_source_chain(&Outer(Inner));
    assert!(chain.contains("error decoding response body"), "{chain}");
    assert!(chain.contains("unexpected EOF"), "{chain}");
    let err = stream_chunk_error("grokv", &chain);
    let s = err.to_string();
    assert!(s.contains("Stream:"), "{s}");
    assert!(s.contains("unexpected EOF"), "{s}");
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
    assert!(matches!(
        stream_usage_from_chunk(&event),
        Some(whycodes_core::types::StreamEvent::Usage {
            input_tokens: 1500,
            output_tokens: 12
        })
    ));
    assert!(stream_usage_from_chunk(&json!({ "choices": [] })).is_none());
    assert!(stream_usage_from_chunk(&json!({ "usage": 1 })).is_none());
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

#[test]
fn json_number_skips_non_finite_and_encodes_finite() {
    assert!(json_number(f64::NAN).is_none());
    assert!(json_number(f64::INFINITY).is_none());
    assert!(json_number(f64::NEG_INFINITY).is_none());
    assert_eq!(json_number(0.5).unwrap(), json!(0.5));

    let mut body = json!({});
    set_json_f64(&mut body, "temperature", f32::NAN);
    assert!(body.get("temperature").is_none());
    apply_sampling(
        &mut body,
        &LlmRequest {
            system: String::new(),
            messages: std::sync::Arc::from(Vec::<Message>::new()),
            tools: std::sync::Arc::from([]),
            max_tokens: None,
            temperature: Some(0.5),
            top_p: Some(0.25),
            top_k: None,
            stop_sequences: None,
            thinking: None,
            use_prompt_cache: false,
        },
    );
    // 0.5 / 0.25 are exact in f32 and f64, so they round-trip as JSON numbers.
    assert_eq!(body["temperature"], json!(0.5));
    assert_eq!(body["top_p"], json!(0.25));
}

#[test]
fn convert_messages_covers_roles_images_tool_result_and_name() {
    let req = LlmRequest {
        system: "sys".into(),
        messages: std::sync::Arc::from(vec![
            Message {
                role: Role::System,
                content: MessageContent::Text("inner-sys".into()),
                tool_call_id: None,
                name: Some("sys-name".into()),
                created_at: None,
            },
            Message {
                role: Role::User,
                content: MessageContent::Blocks(vec![
                    ContentBlock::Text {
                        text: "look".into(),
                    },
                    ContentBlock::Image {
                        source: ImageSource::Base64 {
                            media_type: "image/png".into(),
                            data: "AAAA".into(),
                        },
                    },
                    ContentBlock::Image {
                        source: ImageSource::Url {
                            url: "http://example.invalid/x.png".into(),
                        },
                    },
                    ContentBlock::ToolResult {
                        tool_use_id: "c1".into(),
                        content: "ok".into(),
                        is_error: None,
                    },
                    ContentBlock::RedactedThinking {
                        data: "opaque".into(),
                    },
                ]),
                tool_call_id: None,
                name: Some("user-fn".into()),
                created_at: None,
            },
            Message {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![ContentBlock::Image {
                    source: ImageSource::Url {
                        url: "http://example.invalid/a.png".into(),
                    },
                }]),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
        ]),
        tools: std::sync::Arc::from([]),
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: false,
    };
    let msgs = convert_messages(&req);
    assert_eq!(msgs[0]["role"], "system");
    assert_eq!(msgs[1]["role"], "system");
    assert_eq!(msgs[1]["name"], "sys-name");
    assert_eq!(msgs[2]["role"], "user");
    assert_eq!(msgs[2]["name"], "user-fn");
    let parts = msgs[2]["content"].as_array().unwrap();
    assert!(
        parts.iter().any(
            |p| p["type"] == "text" && p["text"].as_str().unwrap().contains("[tool_result c1]")
        ),
        "{parts:?}"
    );
    assert!(
        parts.iter().any(|p| p["image_url"]["url"]
            .as_str()
            .is_some_and(|u| u.starts_with("data:image/png;base64,"))),
        "{parts:?}"
    );
    assert!(
        parts
            .iter()
            .any(|p| p["image_url"]["url"] == "http://example.invalid/x.png"),
        "{parts:?}"
    );
    assert_eq!(msgs[3]["role"], "assistant");
    assert!(msgs[3]["content"].is_array());
}

#[test]
fn ensure_object_arguments_coerces_non_objects() {
    assert_eq!(ensure_object_arguments(&json!([1, 2])), json!({}));
    assert_eq!(ensure_object_arguments(&json!(7)), json!({}));
    assert_eq!(ensure_object_arguments(&json!("null")), json!({}));
    assert_eq!(
        ensure_object_arguments(&json!("{\"a\":1}")),
        json!({"a": 1})
    );
    assert!(
        arguments_stream_fragment(&json!({"q": "x"}))
            .unwrap()
            .contains("q")
    );
    assert!(arguments_stream_fragment(&json!("")).is_none());
    let events = stream_events_for_tool_call_delta(&json!({
        "index": true,
        "function": { "arguments": "{\"a\":1}" }
    }));
    assert!(matches!(
        &events[0],
        whycodes_core::types::StreamEvent::ToolUseDelta { id, .. } if id == "0"
    ));
    let events = stream_events_for_tool_call_delta(&json!({
        "index": "2",
        "function": { "arguments": "{\"a\":1}" }
    }));
    assert!(matches!(
        &events[0],
        whycodes_core::types::StreamEvent::ToolUseDelta { id, .. } if id == "2"
    ));
    assert_eq!(
        sanitize_schema_for_openai(&json!("not-an-object")),
        json!("not-an-object")
    );
    let mixed = sanitize_schema_for_openai(&json!({
        "properties": "not-an-object",
        "anyOf": "not-an-array"
    }));
    assert_eq!(mixed["properties"], json!("not-an-object"));
    assert_eq!(mixed["anyOf"], json!("not-an-array"));
    assert!(
        reasoning_text_from_delta(&json!({
            "reasoning": { "content": "   " }
        }))
        .is_none()
    );

    #[derive(Debug)]
    struct ChainErr {
        msg: &'static str,
        source: Option<Box<ChainErr>>,
    }
    impl std::fmt::Display for ChainErr {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.msg)
        }
    }
    impl std::error::Error for ChainErr {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.source.as_ref().map(|e| e as _)
        }
    }
    let mut err = ChainErr {
        msg: "leaf",
        source: None,
    };
    for i in 0..10 {
        err = ChainErr {
            msg: Box::leak(format!("e{i}").into_boxed_str()),
            source: Some(Box::new(err)),
        };
    }
    let chain = error_source_chain(&err);
    assert!(chain.contains("e1"), "{chain}");
    assert!(!chain.contains("e0"), "{chain}");
    assert!(!chain.contains("leaf"), "{chain}");
    let blocks = content_blocks_from_chat_message(&json!({
        "reasoning": { "content": "leaf" },
        "content": "hi"
    }));
    assert!(matches!(&blocks[0], ContentBlock::Thinking { text, .. } if text == "leaf"));
}

#[tokio::test]
async fn chat_sse_from_scripted_bytes_covers_pending_error_and_done_break() {
    use futures::StreamExt;
    use std::time::Duration;

    let first = b"data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n".to_vec();
    let rest = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"llo\"}}]}\n",
        "data: [DONE]\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"ignored\"}}]}\n",
    )
    .as_bytes()
    .to_vec();
    let mut stream = chat_sse_from_bytes(
        scripted_bytes([Ok(first), Err("chunk fail".into()), Ok(rest.clone())]),
        "openai",
    );
    let mut text = String::new();
    let mut saw_err = false;
    let mut saw_stop = false;
    while let Some(ev) = stream.next().await {
        match ev {
            Ok(StreamEvent::TextDelta { text: d }) => text.push_str(&d),
            Ok(StreamEvent::MessageStop) => saw_stop = true,
            Err(_) => saw_err = true,
            _ => {}
        }
    }
    assert!(saw_err);
    assert_eq!(text, "hello");
    assert!(saw_stop);

    let mut delayed = chat_sse_from_bytes(
        Box::pin(async_stream::stream! {
            yield Ok(b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n".to_vec());
            tokio::time::sleep(Duration::from_millis(5)).await;
            yield Ok(b"data: [DONE]\nextra-without-newline".to_vec());
        }),
        "openai",
    );
    let mut text = String::new();
    let mut saw_stop = false;
    while let Some(ev) = delayed.next().await {
        match ev.unwrap() {
            StreamEvent::TextDelta { text: d } => text.push_str(&d),
            StreamEvent::MessageStop => saw_stop = true,
            _ => {}
        }
    }
    assert_eq!(text, "hi");
    assert!(saw_stop);
}
