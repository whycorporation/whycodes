use super::{
    AnthropicProvider, anthropic_sse_from_bytes, events_for_data, json_parse_error_for_tests,
    usage_from_message_delta,
};

#[test]
fn json_parse_error_helper_formats_message() {
    let err = json_parse_error_for_tests("eof");
    assert!(err.to_string().contains("JSON parse error"), "{err}");
}
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
    assert!(matches!(&events[0], Ok(StreamEvent::RedactedThinking { data } ) if data == "opaque"));
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

fn serve_once(status: &str, body: &str, content_type: &str) -> String {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let payload = format!("{header}{body}");
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(payload.as_bytes());
        }
    });
    format!("http://{addr}/v1/messages")
}

#[test]
fn usage_zero_zero_is_none_and_unknown_delta_is_silent() {
    let event = json!({
        "type": "message_delta",
        "usage": { "input_tokens": 0, "output_tokens": 0 }
    });
    assert!(usage_from_message_delta(&event).is_none());
    assert!(events_for_data(r#"{"type":"message_start"}"#).is_empty());
    assert!(events_for_data(r#"{"type":"content_block_delta","delta":{"type":"foo"}}"#).is_empty());
}

#[tokio::test]
async fn complete_maps_all_content_block_types() {
    use crate::provider::LlmProvider;
    use whycodes_core::types::ContentBlock;
    let body = json!({
        "content": [
            {"type": "text", "text": "hello"},
            {"type": "tool_use", "id": "t1", "name": "read", "input": {"path": "a.rs"}},
            {"type": "thinking", "thinking": "hmm", "signature": "sig"},
            {"type": "redacted_thinking", "data": "opq"},
            {"type": "server_tool_use"}
        ],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 1, "output_tokens": 2}
    })
    .to_string();
    let p = AnthropicProvider::from_base(Some(&serve_once("200 OK", &body, "application/json")));
    let resp = p.complete(&base_request(), "", "claude").await.unwrap();
    assert!(
        resp.content
            .iter()
            .any(|b| matches!(b, ContentBlock::Text { text } if text == "hello"))
    );
    assert!(
        resp.content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolUse { name, .. } if name == "read"))
    );
    assert!(
        resp.content
            .iter()
            .any(|b| matches!(b, ContentBlock::Thinking { text, .. } if text == "hmm"))
    );
    assert!(
        resp.content
            .iter()
            .any(|b| matches!(b, ContentBlock::RedactedThinking { data } if data == "opq"))
    );
    assert!(
        resp.content
            .iter()
            .any(|b| matches!(b, ContentBlock::Text { text } if text == "[unknown block]"))
    );
}

#[tokio::test]
async fn oauth_token_and_empty_key_complete_against_loopback() {
    use crate::provider::LlmProvider;
    use whycodes_core::types::ContentBlock;
    let body = json!({
        "content": [{"type": "text", "text": "oat"}],
        "usage": {"input_tokens": 1, "output_tokens": 1}
    })
    .to_string();
    let p = AnthropicProvider::from_base(Some(&serve_once("200 OK", &body, "application/json")));
    let resp = p
        .complete(&base_request(), "sk-ant-oat-test", "claude")
        .await
        .unwrap();
    assert!(
        resp.content
            .iter()
            .any(|b| matches!(b, ContentBlock::Text { text } if text == "oat"))
    );
}

#[tokio::test]
async fn stream_parses_sse_and_done() {
    use crate::provider::LlmProvider;
    use tokio_stream::StreamExt;
    let sse = concat!(
        "event: ping\n",
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n",
        "data: [DONE]\n\n",
    );
    let p = AnthropicProvider::from_base(Some(&serve_once("200 OK", sse, "text/event-stream")));
    let mut stream = p.stream(&base_request(), "sk-ant", "claude").await.unwrap();
    let mut text = String::new();
    let mut saw_stop = false;
    while let Some(ev) = stream.next().await {
        match ev.unwrap() {
            StreamEvent::TextDelta { text: d } => text.push_str(&d),
            StreamEvent::MessageStop => saw_stop = true,
            _ => {}
        }
    }
    assert_eq!(text, "hi");
    assert!(saw_stop);
}

#[tokio::test]
async fn sse_from_scripted_bytes_covers_error_pending_and_done_break() {
    use crate::openai_compat::scripted_bytes;
    use futures::StreamExt;
    use std::time::Duration;

    let first = b"data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"he\"}}\n".to_vec();
    let mut stream =
        anthropic_sse_from_bytes(scripted_bytes([Ok(first), Err("chunk fail".into())]));
    let mut text = String::new();
    let mut saw_err = false;
    while let Some(ev) = stream.next().await {
        match ev {
            Ok(StreamEvent::TextDelta { text: d }) => text.push_str(&d),
            Err(_) => saw_err = true,
            _ => {}
        }
    }
    assert_eq!(text, "he");
    assert!(saw_err);

    let rest = concat!(
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n",
        "data: [DONE]\n",
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"x\"}}\n",
    );
    let mut delayed = anthropic_sse_from_bytes(Box::pin(async_stream::stream! {
        yield Ok(rest.as_bytes()[..20].to_vec());
        tokio::time::sleep(Duration::from_millis(5)).await;
        yield Ok(rest.as_bytes()[20..].to_vec());
    }));
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
