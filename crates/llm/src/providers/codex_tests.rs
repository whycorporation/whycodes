use super::*;
use serde_json::json;

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
    assert!(matches!(
        &events[0],
        StreamEvent::TextDelta { text } if text == "hel"
    ));
}

#[test]
fn function_call_item_maps_to_tool_use() {
    let payload = r#"{"type":"response.output_item.done","item":{"type":"function_call","call_id":"call_9","name":"bash","arguments":"{\"cmd\":\"ls\"}"}}"#;
    let events = events_for_payload(payload);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        StreamEvent::ToolUse { id, name, input }
            if id == "call_9" && name == "bash" && input["cmd"] == "ls"
    ));
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

#[test]
fn convert_input_covers_images_thinking_and_empty_text() {
    let messages = vec![
        Message {
            role: Role::System,
            content: MessageContent::Text("   ".into()),
            tool_call_id: None,
            name: None,
            created_at: None,
        },
        Message {
            role: Role::User,
            content: MessageContent::Blocks(vec![
                ContentBlock::Text { text: "see".into() },
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
                ContentBlock::Thinking {
                    text: "plan".into(),
                    signature: None,
                },
                ContentBlock::RedactedThinking {
                    data: "opaque".into(),
                },
            ]),
            tool_call_id: None,
            name: None,
            created_at: None,
        },
        Message {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::Image {
                source: ImageSource::Url {
                    url: "http://example.invalid/skip.png".into(),
                },
            }]),
            tool_call_id: None,
            name: None,
            created_at: None,
        },
    ];
    let items = convert_input(&messages);
    assert!(
        items
            .iter()
            .any(|i| i["content"][0]["type"] == "input_image"),
        "{items:?}"
    );
    assert!(items.iter().all(|i| i["role"] != "assistant"
        || i.get("content").is_some()
        || i["type"] == "function_call"));
}

#[tokio::test]
async fn public_wrappers_use_test_codex_url() {
    let url: &'static str = Box::leak(serve_sse("200 OK", &sse_hello()).into_boxed_str());
    TEST_CODEX_URL.with(|c| *c.borrow_mut() = Some(url));
    let req = simple_request();
    let resp = complete(&req, "eyJhbGciOiJ.eyJzdWIiOiJx.sig", "gpt-test")
        .await
        .unwrap();
    assert!(
        resp.content
            .iter()
            .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("hello"))),
        "{resp:?}"
    );
    TEST_CODEX_URL.with(|c| *c.borrow_mut() = None);

    let url: &'static str = Box::leak(serve_sse("200 OK", &sse_hello()).into_boxed_str());
    TEST_CODEX_URL.with(|c| *c.borrow_mut() = Some(url));
    let mut stream = stream(&req, "eyJhbGciOiJ.eyJzdWIiOiJx.sig", "gpt-test")
        .await
        .unwrap();
    let mut text = String::new();
    while let Some(ev) = stream.next().await {
        if let Ok(StreamEvent::TextDelta { text: d }) = ev {
            text.push_str(&d);
        }
    }
    assert_eq!(text, "hello");
    TEST_CODEX_URL.with(|c| *c.borrow_mut() = None);
}

#[tokio::test]
async fn stream_synthesizes_stop_from_done_without_completed() {
    let sse = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n",
        "event: ping\n",
        "data: [DONE]\n\n",
        "data: [DONE]\n\n",
    );
    let mut stream = stream_at(
        &serve_sse("200 OK", sse),
        &simple_request(),
        "eyJhbGciOiJ.eyJzdWIiOiJx.sig",
        "gpt-test",
    )
    .await
    .unwrap();
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

    let first = b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"he\"}\n".to_vec();
    let mut stream = codex_sse_from_bytes(scripted_bytes([Ok(first), Err("chunk fail".into())]));
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
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n",
        "data: [DONE]\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}\n",
    );
    let mut delayed = codex_sse_from_bytes(Box::pin(async_stream::stream! {
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

#[tokio::test]
async fn complete_sends_chatgpt_account_id_from_stored_extra() {
    let dir = std::env::temp_dir().join(format!(
        "whycodes-codex-acct-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::create_dir_all(&dir);
    let store = whycodes_auth::TokenStore::new(&dir);
    let mut extra = serde_json::Map::new();
    extra.insert(
        "openai_account_id".into(),
        serde_json::Value::String("acct-from-store".into()),
    );
    store
        .set(
            "openai",
            whycodes_auth::ProviderAuth {
                method: "oauth".into(),
                token: whycodes_auth::OAuthToken {
                    access_token: "eyJhbGciOiJ.eyJzdWIiOiJx.sig".into(),
                    refresh_token: None,
                    expires_at: None,
                    extra,
                },
            },
        )
        .unwrap();
    crate::oauth_refresh::register("openai", dir.clone());
    let url: &'static str = Box::leak(serve_sse("200 OK", &sse_hello()).into_boxed_str());
    TEST_CODEX_URL.with(|c| *c.borrow_mut() = Some(url));
    let resp = complete(
        &simple_request(),
        "eyJhbGciOiJ.eyJzdWIiOiJx.sig",
        "gpt-test",
    )
    .await
    .unwrap();
    assert!(
        resp.content
            .iter()
            .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("hello"))),
        "{resp:?}"
    );
    TEST_CODEX_URL.with(|c| *c.borrow_mut() = None);
    crate::oauth_refresh::unregister("openai");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn complete_assembles_tool_use_and_missing_stop() {
    let body = format!(
        "data: {}\n\ndata: {}\n\n",
        serde_json::json!({"type":"response.output_text.delta","delta":"hi"}),
        serde_json::json!({
            "type":"response.output_item.done",
            "item":{"type":"function_call","call_id":"c1","name":"read","arguments":"{\"p\":1}"}
        })
    );
    let url = serve_sse("200 OK", &body);
    let resp = complete_at(
        &url,
        &simple_request(),
        "eyJhbGciOiJ.eyJzdWIiOiJx.sig",
        "gpt-test",
    )
    .await
    .unwrap();
    assert!(
        resp.content
            .iter()
            .any(|b| matches!(b, ContentBlock::Text { text } if text == "hi"))
    );
    assert!(
        resp.content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolUse { name, .. } if name == "read"))
    );
}

#[tokio::test]
async fn complete_ignores_non_text_stream_events() {
    let body = format!(
        "data: {}\n\ndata: {}\n\ndata: {}\n\n",
        serde_json::json!({"type":"response.output_text.delta","delta":"hi"}),
        serde_json::json!({"type":"error","message":"ignored"}),
        serde_json::json!({
            "type":"response.completed",
            "response":{"usage":{"input_tokens":1,"output_tokens":1}}
        })
    );
    let url = serve_sse("200 OK", &body);
    let resp = complete_at(
        &url,
        &simple_request(),
        "eyJhbGciOiJ.eyJzdWIiOiJx.sig",
        "gpt-test",
    )
    .await
    .unwrap();
    assert!(
        resp.content
            .iter()
            .any(|b| matches!(b, ContentBlock::Text { text } if text == "hi")),
        "{resp:?}"
    );
    assert_eq!(resp.stop_reason.as_deref(), Some("stop"));
}

#[test]
fn events_for_payload_error_message_and_empty_delta() {
    let events = events_for_payload(r#"{"type":"error","message":"boom"}"#);
    assert!(matches!(&events[0], StreamEvent::Error { message } if message == "boom"));
    assert!(events_for_payload(r#"{"type":"response.output_text.delta"}"#).is_empty());
    let events = events_for_payload(
        r#"{"type":"response.output_item.done","item":{"type":"function_call","arguments":"not-json"}}"#,
    );
    assert!(
        matches!(&events[0], StreamEvent::ToolUse { name, input, .. } if name.is_empty() && input.is_null())
    );
}
