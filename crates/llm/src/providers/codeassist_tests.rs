use super::*;
use futures::StreamExt;
use serde_json::json;
use whycodes_core::types::{ImageSource, Message, ToolDefinition};

fn message(role: Role, content: MessageContent) -> Message {
    Message {
        role,
        content,
        tool_call_id: None,
        name: None,
        created_at: None,
    }
}

fn empty_request(messages: Vec<Message>) -> LlmRequest {
    LlmRequest {
        system: String::new(),
        messages: std::sync::Arc::from(messages),
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
fn client_metadata_matches_code_assist_contract() {
    assert_eq!(
        client_metadata(),
        json!({
            "ideType": "IDE_UNSPECIFIED",
            "platform": "PLATFORM_UNSPECIFIED",
            "pluginType": "GEMINI",
        })
    );
    assert_eq!(
        metadata_for(&ANTIGRAVITY),
        json!({ "ideType": "ANTIGRAVITY" })
    );
}

#[test]
fn antigravity_envelope_tags_native_client() {
    let request = empty_request(vec![message(
        Role::User,
        MessageContent::Text("hi".to_string()),
    )]);
    let mut inner = build_inner_request(&history_with_tool_call());
    apply_antigravity_inner(&mut inner, &history_with_tool_call());
    assert_eq!(inner["systemInstruction"]["role"], "user");
    assert_eq!(
        inner["toolConfig"]["functionCallingConfig"]["mode"],
        "VALIDATED"
    );

    let body = wrap_envelope(&ANTIGRAVITY, "gemini-3.1-pro-low", "proj", inner, &request);
    assert_eq!(body["userAgent"], "antigravity");
    assert_eq!(body["requestType"], "agent");
    assert_eq!(body["project"], "proj");
    assert_eq!(body["model"], "gemini-3.1-pro-low");
    assert!(
        body["requestId"]
            .as_str()
            .is_some_and(|id| id.starts_with("agent/")),
        "requestId={}",
        body["requestId"]
    );
    assert!(body["request"]["sessionId"].as_str().is_some());

    let gemini = wrap_envelope(
        &GEMINI_CLI,
        "gemini-2.5-pro",
        "g",
        build_inner_request(&request),
        &request,
    );
    assert!(gemini.get("userAgent").is_none());
    assert!(gemini.get("requestType").is_none());
}

#[test]
fn antigravity_wire_model_collapses_family_ids() {
    assert_eq!(
        antigravity_wire_model("gemini-3.1-pro"),
        "gemini-3.1-pro-low"
    );
    assert_eq!(
        antigravity_wire_model("gemini-3.1-flash"),
        "gemini-3.5-flash-low"
    );
    assert_eq!(
        antigravity_wire_model("claude-sonnet-4-6"),
        "claude-sonnet-4-6"
    );
    assert_eq!(
        antigravity_wire_model("gemini-3.1-pro-low"),
        "gemini-3.1-pro-low"
    );
}

#[test]
fn token_shape_detection() {
    assert!(is_google_oauth_token("ya29.a0AfH6SMBx…"));
    assert!(!is_google_oauth_token("AIzaSyAbc123"));
    assert!(!is_google_oauth_token(""));
}

fn history_with_tool_call() -> LlmRequest {
    LlmRequest {
        system: "sys".to_string(),
        messages: std::sync::Arc::from(vec![
            Message {
                role: Role::User,
                content: MessageContent::Text("run ls".to_string()),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
            Message {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                    id: "gcall_1".to_string(),
                    name: "bash".to_string(),
                    input: json!({"cmd": "ls"}),
                }]),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
            Message {
                role: Role::User,
                content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                    tool_use_id: "gcall_1".to_string(),
                    content: "a.rs".to_string(),
                    is_error: None,
                }]),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
        ]),
        tools: vec![ToolDefinition {
            name: "bash".to_string(),
            description: "Run a command".to_string(),
            parameters: json!({"type": "object", "properties": {}}),
        }]
        .into(),
        max_tokens: Some(1024),
        temperature: Some(0.5),
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: true,
    }
}

#[test]
fn inner_request_maps_function_parts() {
    let inner = build_inner_request(&history_with_tool_call());
    assert_eq!(inner["systemInstruction"]["parts"][0]["text"], "sys");
    assert_eq!(inner["generationConfig"]["maxOutputTokens"], 1024);
    assert_eq!(inner["tools"][0]["functionDeclarations"][0]["name"], "bash");

    let contents = inner["contents"].as_array().unwrap();
    assert_eq!(contents[0]["role"], "user");
    assert_eq!(contents[1]["role"], "model");
    assert_eq!(contents[1]["parts"][0]["functionCall"]["name"], "bash");
    assert_eq!(contents[1]["parts"][0]["functionCall"]["args"]["cmd"], "ls");
    // ToolResult must be matched back to its tool *name* via the id map.
    assert_eq!(contents[2]["parts"][0]["functionResponse"]["name"], "bash");
    assert_eq!(
        contents[2]["parts"][0]["functionResponse"]["response"]["result"],
        "a.rs"
    );
}

#[test]
fn inner_request_omits_empty_optional_sections_and_unsupported_blocks() {
    let request = empty_request(vec![
        message(Role::System, MessageContent::Text("   ".to_string())),
        message(
            Role::Assistant,
            MessageContent::Blocks(vec![
                ContentBlock::Image {
                    source: ImageSource::Base64 {
                        media_type: "image/png".to_string(),
                        data: "AAAA".to_string(),
                    },
                },
                ContentBlock::Thinking {
                    text: "private".to_string(),
                    signature: None,
                },
                ContentBlock::RedactedThinking {
                    data: "opaque".to_string(),
                },
            ]),
        ),
    ]);

    let inner = build_inner_request(&request);
    let parts = &inner["contents"][0]["parts"];
    assert_eq!(parts[0]["thought"], true);
    assert_eq!(parts[0]["text"], "private");
    assert_eq!(parts[1]["thought"], true);
    assert_eq!(parts[1]["text"], "opaque");
    assert_eq!(inner["contents"][0]["role"], "model");
}

#[test]
fn inner_request_echoes_thought_signature() {
    let request = empty_request(vec![message(
        Role::Assistant,
        MessageContent::Blocks(vec![
            ContentBlock::Thinking {
                text: "plan".to_string(),
                signature: Some("sig-9".to_string()),
            },
            ContentBlock::Text {
                text: "ok".to_string(),
            },
        ]),
    )]);
    let inner = build_inner_request(&request);
    assert_eq!(inner["contents"][0]["parts"][0]["thought"], true);
    assert_eq!(
        inner["contents"][0]["parts"][0]["thoughtSignature"],
        "sig-9"
    );
    assert_eq!(inner["contents"][0]["parts"][1]["text"], "ok");
    assert!(inner["contents"][0]["parts"][1].get("thought").is_none());
}

#[test]
fn inner_request_uses_fallback_name_for_unmatched_tool_result() {
    let request = empty_request(vec![message(
        Role::Tool,
        MessageContent::Blocks(vec![ContentBlock::ToolResult {
            tool_use_id: "missing-call".to_string(),
            content: "failed".to_string(),
            is_error: Some(true),
        }]),
    )]);

    let inner = build_inner_request(&request);
    assert_eq!(inner["contents"][0]["role"], "user");
    assert_eq!(
        inner["contents"][0]["parts"][0]["functionResponse"],
        json!({"name": "tool", "response": {"result": "failed"}})
    );
    assert!(inner.get("systemInstruction").is_none());
    assert!(inner.get("tools").is_none());
    assert!(inner.get("generationConfig").is_none());
}

#[test]
fn chunk_maps_text_function_call_and_usage() {
    let mut seq = 0u64;
    let events = events_for_chunk(
        r#"{"candidates":[{"content":{"parts":[{"text":"hi "},{"functionCall":{"name":"read","args":{"path":"x"}}}]}}],"usageMetadata":{"promptTokenCount":7,"candidatesTokenCount":3}}"#,
        &mut seq,
    );
    assert!(matches!(&events[0], StreamEvent::TextDelta { text } if text == "hi "));
    assert!(matches!(
        &events[1],
        StreamEvent::ToolUse { id, name, input }
            if id == "gcall_1" && name == "read" && input["path"] == "x"
    ));
    assert!(matches!(
        events[2],
        StreamEvent::Usage {
            input_tokens: 7,
            output_tokens: 3
        }
    ));
    assert!(matches!(events[3], StreamEvent::MessageStop));
}

#[test]
fn chunk_maps_thought_parts_not_visible_text() {
    let mut seq = 0u64;
    let events = events_for_chunk(
        r#"{"candidates":[{"content":{"parts":[{"text":"hmm","thought":true,"thoughtSignature":"sig"},{"text":"hi"}]}}]}"#,
        &mut seq,
    );
    assert!(
        matches!(&events[0], StreamEvent::ThinkingSignature { signature } if signature == "sig")
    );
    assert!(matches!(&events[1], StreamEvent::Thinking { text } if text == "hmm"));
    assert!(matches!(&events[2], StreamEvent::TextDelta { text } if text == "hi"));
}

#[test]
fn http_error_unwraps_nested_anthropic_message() {
    let json = json!({
        "error": {
            "code": 400,
            "message": "{\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\"message\":\"messages.2: The final block in an assistant message cannot be `thinking`.\"}}",
            "status": "INVALID_ARGUMENT"
        }
    });
    let s = code_assist_http_error(
        reqwest::StatusCode::BAD_REQUEST,
        "claude-sonnet-4-6",
        &json,
        &json.to_string(),
    );
    assert!(s.contains("claude-sonnet-4-6"), "{s}");
    assert!(s.contains("cannot be `thinking`"), "{s}");
    assert!(!s.contains("INVALID_ARGUMENT"), "{s}");
}

#[test]
fn chunk_tolerates_response_wrapper() {
    let mut seq = 0u64;
    let events = events_for_chunk(
        r#"{"response":{"candidates":[{"content":{"parts":[{"text":"wrapped"}]}}]}}"#,
        &mut seq,
    );
    assert!(matches!(&events[0], StreamEvent::TextDelta { text } if text == "wrapped"));
}

#[test]
fn malformed_and_structurally_empty_chunks_are_ignored() {
    let mut seq = 41u64;
    assert!(events_for_chunk("not json", &mut seq).is_empty());
    assert!(events_for_chunk(r#"{"candidates":null}"#, &mut seq).is_empty());
    assert_eq!(seq, 41);
}

#[test]
fn chunk_defaults_missing_call_fields_and_usage_counts() {
    let mut seq = 7u64;
    let events = events_for_chunk(
        r#"{"candidates":[{"content":{"parts":[{"functionCall":{}}]},"finishReason":"MAX_TOKENS"}],"usageMetadata":{}}"#,
        &mut seq,
    );

    assert!(matches!(
        &events[0],
        StreamEvent::ToolUse { id, name, input }
            if id == "gcall_8" && name.is_empty() && input.is_null()
    ));
    assert!(matches!(
        &events[1],
        StreamEvent::MessageDelta { delta }
            if delta == &json!({"finishReason": "MAX_TOKENS"})
    ));
    assert!(matches!(
        events[2],
        StreamEvent::Usage {
            input_tokens: 0,
            output_tokens: 0
        }
    ));
    assert!(matches!(events[3], StreamEvent::MessageStop));
    assert_eq!(seq, 8);
}

#[test]
fn production_onboard_poll_sleep_is_one_second() {
    TEST_POLL_SLEEP.with(|c| c.set(Duration::MAX));
    assert_eq!(onboard_poll_sleep(), Duration::from_secs(1));
    TEST_POLL_SLEEP.with(|c| c.set(Duration::ZERO));
}

#[test]
fn tier_picking() {
    let load = json!({"allowedTiers": [
        {"id": "free-tier", "userDefinedCloudaicompanionProject": false},
        {"id": "standard-tier", "userDefinedCloudaicompanionProject": true}
    ]});
    assert_eq!(pick_tier(&load, false), "free-tier");
    assert_eq!(pick_tier(&load, true), "standard-tier");
    // No tier list: sane defaults.
    let empty = json!({});
    assert_eq!(pick_tier(&empty, false), "free-tier");
    assert_eq!(pick_tier(&empty, true), "standard-tier");

    // Matching entries without string ids are unusable and fall back.
    let malformed = json!({"allowedTiers": [
        {"id": null, "userDefinedCloudaicompanionProject": false},
        {"id": 7, "userDefinedCloudaicompanionProject": true}
    ]});
    assert_eq!(pick_tier(&malformed, false), "free-tier");
    assert_eq!(pick_tier(&malformed, true), "standard-tier");
}

fn serve_once(status: &str, body: &str, content_type: &str) -> String {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;
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
            thread::sleep(Duration::from_millis(20));
        }
    });
    format!("http://{addr}/v1internal")
}

fn loopback_profile(base: String, provider: &'static str) -> Profile {
    let url: &'static str = Box::leak(base.into_boxed_str());
    let bases: &'static [&'static str] = Box::leak(Box::new([url]));
    Profile {
        oauth_provider: provider,
        bases,
        antigravity: false,
    }
}

fn assist_request() -> LlmRequest {
    empty_request(vec![message(
        Role::User,
        MessageContent::Text("hi".to_string()),
    )])
}

#[tokio::test]
async fn complete_and_stream_against_loopback() {
    cache_project("google-loopback-ok", "proj-test");
    let json = serde_json::json!({
        "candidates": [{
            "content": {"parts": [{"text": "hello-assist"}]},
            "finishReason": "STOP"
        }],
        "usageMetadata": {"promptTokenCount": 1, "candidatesTokenCount": 2}
    })
    .to_string();
    let profile = loopback_profile(
        serve_once("200 OK", &json, "application/json"),
        "google-loopback-ok",
    );
    let req = assist_request();
    let resp = complete_with(&profile, &req, "ya29.test", "gemini-test")
        .await
        .unwrap();
    assert!(
        resp.content.iter().any(|b| matches!(
            b,
            ContentBlock::Text { text } if text.contains("hello-assist")
        )),
        "{resp:?}"
    );

    cache_project("google-loopback-err", "proj-test");
    let err_p = loopback_profile(
        serve_once(
            "403 Forbidden",
            r#"{"error":{"message":"nope"}}"#,
            "application/json",
        ),
        "google-loopback-err",
    );
    let err = complete_with(&err_p, &req, "ya29.bad", "gemini-test")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("nope") || !err.to_string().is_empty(),
        "{err}"
    );

    cache_project("google-loopback-sse", "proj-test");
    let sse = format!(
        "data: {}\n\n",
        serde_json::json!({
            "candidates": [{"content": {"parts": [{"text": "hello"}]}}]
        })
    );
    let sse_p = loopback_profile(
        serve_once("200 OK", &sse, "text/event-stream"),
        "google-loopback-sse",
    );
    let mut stream = stream_with(&sse_p, &req, "ya29.test", "gemini-test")
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

fn serve_seq(parts: Vec<(String, String, String)>) -> String {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for (status, body, ct) in parts {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let header = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(format!("{header}{body}").as_bytes());
                thread::sleep(Duration::from_millis(5));
            }
        }
    });
    format!("http://{addr}/v1internal")
}

fn leak_url(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn leak_bases(urls: Vec<&'static str>) -> &'static [&'static str] {
    Box::leak(urls.into_boxed_slice())
}

fn unique_provider(label: &str) -> &'static str {
    Box::leak(
        format!(
            "google-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        )
        .into_boxed_str(),
    )
}

fn generate_ok_json() -> String {
    serde_json::json!({
        "candidates": [{
            "content": {"parts": [
                {"text": "hello-assist"},
                {"functionCall": {"name": "read", "args": {"path": "a.rs"}}}
            ]},
            "finishReason": "STOP"
        }],
        "usageMetadata": {"promptTokenCount": 1, "candidatesTokenCount": 2}
    })
    .to_string()
}

#[tokio::test]
async fn empty_bases_errors_before_http() {
    cache_project("google-empty-bases", "p");
    let profile = Profile {
        oauth_provider: "google-empty-bases",
        bases: &[],
        antigravity: false,
    };
    let err = complete_with(&profile, &assist_request(), "ya29.x", "m")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no endpoints"), "{err}");
}

#[tokio::test]
async fn load_code_assist_resolves_project_then_generates() {
    let provider = unique_provider("load");
    let load = serde_json::json!({"cloudaicompanionProject": "from-load"}).to_string();
    let wrapped = serde_json::json!({
        "response": {
            "candidates": [{"content": {"parts": [{"text": "wrapped"}]}}],
            "usageMetadata": {"promptTokenCount": 3, "candidatesTokenCount": 4}
        }
    })
    .to_string();
    let profile = loopback_profile(
        serve_seq(vec![
            ("200 OK".into(), load, "application/json".into()),
            ("200 OK".into(), wrapped, "application/json".into()),
        ]),
        provider,
    );
    let resp = complete_with(&profile, &assist_request(), "ya29.x", "gemini-test")
        .await
        .unwrap();
    assert!(
        resp.content
            .iter()
            .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("wrapped"))),
        "{resp:?}"
    );
    assert_eq!(cached_project(provider).as_deref(), Some("from-load"));
}

#[tokio::test]
async fn load_current_tier_uses_default_project() {
    let provider = unique_provider("tier");
    let load = serde_json::json!({"currentTier": {"id": "paid"}}).to_string();
    let profile = loopback_profile(
        serve_seq(vec![
            ("200 OK".into(), load, "application/json".into()),
            (
                "200 OK".into(),
                generate_ok_json(),
                "application/json".into(),
            ),
        ]),
        provider,
    );
    let resp = complete_with(&profile, &assist_request(), "ya29.x", "gemini-test")
        .await
        .unwrap();
    assert!(
        resp.content.iter().any(|b| matches!(
            b,
            ContentBlock::Text { text } if text.contains("hello-assist")
        )),
        "{resp:?}"
    );
    assert!(matches!(
        &resp.content[1],
        ContentBlock::ToolUse { name, .. } if name == "read"
    ));
    assert_eq!(cached_project(provider).as_deref(), Some("whycodes"));
}

#[tokio::test]
async fn onboard_polls_until_done() {
    let provider = unique_provider("onboard");
    TEST_POLL_SLEEP.with(|c| c.set(Duration::from_millis(1)));
    let load = serde_json::json!({"allowedTiers": [
        {"id": "free-tier", "userDefinedCloudaicompanionProject": false}
    ]})
    .to_string();
    let onboard = serde_json::json!({"name": "operations/abc", "done": false}).to_string();
    let poll = serde_json::json!({
        "done": true,
        "response": {"cloudaicompanionProject": {"id": "proj-onboard"}}
    })
    .to_string();
    let profile = Profile {
        oauth_provider: provider,
        bases: leak_bases(vec![leak_url(serve_seq(vec![
            ("200 OK".into(), load, "application/json".into()),
            ("200 OK".into(), onboard, "application/json".into()),
            ("200 OK".into(), poll, "application/json".into()),
            (
                "200 OK".into(),
                generate_ok_json(),
                "application/json".into(),
            ),
        ]))]),
        antigravity: true,
    };
    let resp = complete_with(&profile, &assist_request(), "ya29.x", "gemini-3.1-pro")
        .await
        .unwrap();
    assert!(
        resp.content.iter().any(|b| matches!(
            b,
            ContentBlock::Text { text } if text.contains("hello-assist")
        )),
        "{resp:?}"
    );
    assert_eq!(cached_project(provider).as_deref(), Some("proj-onboard"));
    TEST_POLL_SLEEP.with(|c| c.set(Duration::ZERO));
}

#[tokio::test]
async fn load_and_onboard_http_errors() {
    let provider = unique_provider("load-err");
    let err_p = loopback_profile(
        serve_once(
            "403 Forbidden",
            r#"{"error":{"message":"load-nope"}}"#,
            "application/json",
        ),
        provider,
    );
    let err = complete_with(&err_p, &assist_request(), "ya29.x", "m")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("load-nope") || err.to_string().contains("403"),
        "{err}"
    );

    let provider = unique_provider("onboard-err");
    let load = serde_json::json!({"allowedTiers": []}).to_string();
    let onboard_err = r#"{"error":{"message":"onboard-nope"}}"#.to_string();
    let err_p = Profile {
        oauth_provider: provider,
        bases: leak_bases(vec![leak_url(serve_seq(vec![
            ("200 OK".into(), load, "application/json".into()),
            (
                "400 Bad Request".into(),
                onboard_err,
                "application/json".into(),
            ),
        ]))]),
        antigravity: true,
    };
    let err = complete_with(&err_p, &assist_request(), "ya29.x", "m")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("onboard-nope") || err.to_string().contains("onboardUser"),
        "{err}"
    );
}

#[tokio::test]
async fn antigravity_failsover_then_succeeds() {
    let provider = unique_provider("fail-over");
    cache_project(provider, "proj-test");
    let fail = leak_url(serve_once("503 Service Unavailable", "down", "text/plain"));
    let ok = leak_url(serve_once(
        "200 OK",
        &generate_ok_json(),
        "application/json",
    ));
    let profile = Profile {
        oauth_provider: provider,
        bases: leak_bases(vec![fail, ok]),
        antigravity: true,
    };
    let resp = complete_with(&profile, &assist_request(), "ya29.x", "gemini-3.1-flash")
        .await
        .unwrap();
    assert!(
        resp.content.iter().any(|b| matches!(
            b,
            ContentBlock::Text { text } if text.contains("hello-assist")
        )),
        "{resp:?}"
    );
}

#[tokio::test]
async fn complete_maps_function_call_parts() {
    let provider = unique_provider("fn-call");
    cache_project(provider, "proj-test");
    let json = serde_json::json!({
        "candidates": [{
            "content": {"parts": [
                {"text": "calling"},
                {"functionCall": {"name": "read", "args": {"p": 1}}}
            ]},
            "finishReason": "STOP"
        }],
        "usageMetadata": {"promptTokenCount": 1, "candidatesTokenCount": 1}
    })
    .to_string();
    let profile = loopback_profile(serve_once("200 OK", &json, "application/json"), provider);
    let resp = complete_with(&profile, &assist_request(), "ya29.x", "gemini-test")
        .await
        .unwrap();
    assert!(
        resp.content.iter().any(|b| matches!(
            b,
            ContentBlock::ToolUse { name, .. } if name == "read"
        )),
        "{resp:?}"
    );
}

#[tokio::test]
async fn generate_errors_when_no_endpoints_configured() {
    let provider = unique_provider("no-bases");
    cache_project(provider, "proj-test");
    let profile = Profile {
        oauth_provider: provider,
        bases: &[],
        antigravity: false,
    };
    let err = complete_with(&profile, &assist_request(), "ya29.x", "m")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no endpoints configured"), "{err}");
}

#[tokio::test]
async fn stream_http_error_and_public_wrappers() {
    cache_project("google", "proj-public");
    cache_project("google-antigravity", "proj-public");
    let json = generate_ok_json();
    let gemini = leak_url(serve_once("200 OK", &json, "application/json"));
    TEST_GEMINI_BASES.with(|c| *c.borrow_mut() = Some(leak_bases(vec![gemini])));
    let resp = complete(&assist_request(), "ya29.public", "gemini-test")
        .await
        .unwrap();
    assert!(
        resp.content.iter().any(|b| matches!(
            b,
            ContentBlock::Text { text } if text.contains("hello-assist")
        )),
        "{resp:?}"
    );
    TEST_GEMINI_BASES.with(|c| *c.borrow_mut() = None);

    let sse = format!(
        "data: {}\n\n",
        serde_json::json!({
            "candidates": [{"content": {"parts": [{"text": "hello"}]}}]
        })
    );
    let anti = leak_url(serve_once("200 OK", &sse, "text/event-stream"));
    TEST_ANTIGRAVITY_BASES.with(|c| *c.borrow_mut() = Some(leak_bases(vec![anti])));
    let mut anti_stream = stream_antigravity(&assist_request(), "ya29.public", "gemini-3.1-pro")
        .await
        .unwrap();
    let mut text = String::new();
    while let Some(ev) = anti_stream.next().await {
        if let Ok(StreamEvent::TextDelta { text: d }) = ev {
            text.push_str(&d);
        }
    }
    assert_eq!(text, "hello");
    TEST_ANTIGRAVITY_BASES.with(|c| *c.borrow_mut() = None);

    let err_url = leak_url(serve_once(
        "403 Forbidden",
        r#"{"error":{"message":"stream-nope"}}"#,
        "application/json",
    ));
    TEST_GEMINI_BASES.with(|c| *c.borrow_mut() = Some(leak_bases(vec![err_url])));
    let err = super::stream(&assist_request(), "ya29.public", "gemini-test")
        .await
        .map(|_| ())
        .unwrap_err();
    assert!(
        err.to_string().contains("stream-nope") || !err.to_string().is_empty(),
        "{err}"
    );
    TEST_GEMINI_BASES.with(|c| *c.borrow_mut() = None);

    let anti_json = leak_url(serve_once("200 OK", &json, "application/json"));
    TEST_ANTIGRAVITY_BASES.with(|c| *c.borrow_mut() = Some(leak_bases(vec![anti_json])));
    let resp = complete_antigravity(&assist_request(), "ya29.public", "gemini-3.6-flash")
        .await
        .unwrap();
    assert!(
        resp.content.iter().any(|b| matches!(
            b,
            ContentBlock::Text { text } if text.contains("hello-assist")
        )),
        "{resp:?}"
    );
    TEST_ANTIGRAVITY_BASES.with(|c| *c.borrow_mut() = None);
}

#[tokio::test]
async fn stream_skips_empty_sse_lines_and_stops_without_done() {
    cache_project("google", "proj-sse-empty");
    let sse = concat!(
        ": keep-alive\n",
        "\n",
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"ok\"}]}}]}\n\n",
    );
    let url = leak_url(serve_once("200 OK", sse, "text/event-stream"));
    TEST_GEMINI_BASES.with(|c| *c.borrow_mut() = Some(leak_bases(vec![url])));
    let mut stream = super::stream(&assist_request(), "ya29.public", "gemini-test")
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
    TEST_GEMINI_BASES.with(|c| *c.borrow_mut() = None);
    assert_eq!(text, "ok");
    assert!(saw_stop);
}

#[test]
fn remaining_wire_models_and_http_error_fallbacks() {
    assert_eq!(
        antigravity_wire_model("gemini-3.1-pro-high"),
        "gemini-pro-agent"
    );
    assert_eq!(
        antigravity_wire_model("gemini-3-flash"),
        "gemini-3.5-flash-low"
    );
    assert_eq!(
        antigravity_wire_model("gemini-3.5-flash"),
        "gemini-3.5-flash-low"
    );
    assert_eq!(
        antigravity_wire_model("gemini-3.6-flash"),
        "gemini-3.6-flash-low"
    );
    assert_eq!(
        antigravity_wire_model("gemini-3.7-flash"),
        "gemini-3.7-flash-low"
    );
    let empty = code_assist_http_error(
        reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        "m",
        &json!({}),
        "   ",
    );
    assert!(empty.contains("Unknown error"), "{empty}");
    let raw = code_assist_http_error(
        reqwest::StatusCode::BAD_REQUEST,
        "m",
        &json!({}),
        "plain-body-reason",
    );
    assert!(raw.contains("plain-body-reason"), "{raw}");
    assert!(failover_http(404));
    assert!(failover_http(429));
    assert!(failover_http(504));
    assert!(!failover_http(400));
    assert!(!failover_http(401));
    let from_field = code_assist_http_error(
        reqwest::StatusCode::BAD_REQUEST,
        "m",
        &json!({"error": {"message": "plain not json"}}),
        "",
    );
    assert!(from_field.contains("plain not json"), "{from_field}");
    TEST_GEMINI_BASES.with(|c| *c.borrow_mut() = None);
    TEST_ANTIGRAVITY_BASES.with(|c| *c.borrow_mut() = None);
    assert_eq!(gemini_cli_profile().oauth_provider, "google");
    assert!(!gemini_cli_profile().antigravity);
    assert_eq!(antigravity_profile().oauth_provider, "google-antigravity");
    assert!(antigravity_profile().antigravity);
    let mut seq = 0u64;
    let empty_parts = events_for_chunk(r#"{"candidates":[{"content":{}}]}"#, &mut seq);
    assert!(empty_parts.is_empty());
}

#[tokio::test]
async fn sse_from_scripted_bytes_covers_error_pending_and_done_break() {
    use crate::openai_compat::scripted_bytes;
    use futures::StreamExt;
    use std::time::Duration;

    let first = format!(
        "data: {}\n",
        serde_json::json!({"candidates":[{"content":{"parts":[{"text":"he"}]}}]})
    )
    .into_bytes();
    let mut stream =
        codeassist_sse_from_bytes(scripted_bytes([Ok(first), Err("chunk fail".into())]));
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

    let rest = format!(
        "data: {}\nleftover",
        serde_json::json!({
            "candidates":[{"content":{"parts":[{"text":"hi"}]}}],
            "usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":1}
        })
    );
    let extra = format!(
        "data: {}\n",
        serde_json::json!({"candidates":[{"content":{"parts":[{"text":"x"}]}}]})
    );
    let payload = format!("{rest}{extra}");
    let mut delayed = codeassist_sse_from_bytes(Box::pin(async_stream::stream! {
        yield Ok(payload.as_bytes()[..20].to_vec());
        tokio::time::sleep(Duration::from_millis(5)).await;
        yield Ok(payload.as_bytes()[20..].to_vec());
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
async fn stored_extra_project_id_is_cached_without_prior_cache() {
    let provider = unique_provider("stored-extra");
    let dir = std::env::temp_dir().join(format!(
        "whycodes-codeassist-extra-{}-{}",
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
        "project_id".into(),
        serde_json::Value::String("proj-from-extra".into()),
    );
    store
        .set(
            provider,
            whycodes_auth::ProviderAuth {
                method: "oauth".into(),
                token: whycodes_auth::OAuthToken {
                    access_token: "ya29.x".into(),
                    refresh_token: None,
                    expires_at: None,
                    extra,
                },
            },
        )
        .unwrap();
    crate::oauth_refresh::register(provider, dir.clone());
    let profile = loopback_profile(
        serve_once("200 OK", &generate_ok_json(), "application/json"),
        provider,
    );
    let resp = complete_with(&profile, &assist_request(), "ya29.x", "gemini-test")
        .await
        .unwrap();
    assert!(
        resp.content.iter().any(|b| matches!(
            b,
            ContentBlock::Text { text } if text.contains("hello-assist")
        )),
        "{resp:?}"
    );
    assert_eq!(cached_project(provider).as_deref(), Some("proj-from-extra"));
    crate::oauth_refresh::unregister(provider);
    let _ = std::fs::remove_dir_all(&dir);
}

fn google_cloud_project_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[tokio::test]
async fn onboard_without_name_errors_when_no_project() {
    let _guard = google_cloud_project_lock();
    let prev = std::env::var("GOOGLE_CLOUD_PROJECT").ok();
    unsafe {
        std::env::remove_var("GOOGLE_CLOUD_PROJECT");
    }
    let provider = unique_provider("onboard-noname");
    let load = serde_json::json!({"allowedTiers": [
        {"id": "free-tier", "userDefinedCloudaicompanionProject": false}
    ]})
    .to_string();
    let onboard = serde_json::json!({"done": false}).to_string();
    let profile = Profile {
        oauth_provider: provider,
        bases: leak_bases(vec![leak_url(serve_seq(vec![
            ("200 OK".into(), load, "application/json".into()),
            ("200 OK".into(), onboard, "application/json".into()),
        ]))]),
        antigravity: true,
    };
    let err = complete_with(&profile, &assist_request(), "ya29.x", "m")
        .await
        .unwrap_err();
    match prev {
        Some(v) => unsafe { std::env::set_var("GOOGLE_CLOUD_PROJECT", v) },
        None => unsafe { std::env::remove_var("GOOGLE_CLOUD_PROJECT") },
    }
    assert!(
        err.to_string().contains("GOOGLE_CLOUD_PROJECT") || err.to_string().contains("project"),
        "{err}"
    );
}

#[tokio::test]
async fn onboard_includes_env_project_for_antigravity() {
    let _guard = google_cloud_project_lock();
    let provider = unique_provider("onboard-env");
    let prev = std::env::var("GOOGLE_CLOUD_PROJECT").ok();
    unsafe {
        std::env::set_var("GOOGLE_CLOUD_PROJECT", "env-proj");
    }
    let load = serde_json::json!({"allowedTiers": [
        {"id": "standard-tier", "userDefinedCloudaicompanionProject": true}
    ]})
    .to_string();
    let onboard = serde_json::json!({
        "done": true,
        "response": {"cloudaicompanionProject": {"id": "from-onboard"}}
    })
    .to_string();
    let profile = Profile {
        oauth_provider: provider,
        bases: leak_bases(vec![leak_url(serve_seq(vec![
            ("200 OK".into(), load, "application/json".into()),
            ("200 OK".into(), onboard, "application/json".into()),
            (
                "200 OK".into(),
                generate_ok_json(),
                "application/json".into(),
            ),
        ]))]),
        antigravity: true,
    };
    let resp = complete_with(&profile, &assist_request(), "ya29.x", "gemini-3.1-flash")
        .await
        .unwrap();
    assert!(
        resp.content.iter().any(|b| matches!(
            b,
            ContentBlock::Text { text } if text.contains("hello-assist")
        )),
        "{resp:?}"
    );
    match prev {
        Some(v) => unsafe { std::env::set_var("GOOGLE_CLOUD_PROJECT", v) },
        None => unsafe { std::env::remove_var("GOOGLE_CLOUD_PROJECT") },
    }
}

#[tokio::test]
async fn complete_candidates_without_parts_are_empty() {
    let provider = unique_provider("no-parts");
    cache_project(provider, "proj-test");
    let json = serde_json::json!({
        "candidates": [{"content": {}}],
        "usageMetadata": {"promptTokenCount": 1, "candidatesTokenCount": 0}
    })
    .to_string();
    let profile = loopback_profile(serve_once("200 OK", &json, "application/json"), provider);
    let resp = complete_with(&profile, &assist_request(), "ya29.x", "gemini-test")
        .await
        .unwrap();
    assert!(resp.content.is_empty(), "{resp:?}");
}
