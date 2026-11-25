use super::*;
use crate::provider::LlmProvider;

#[test]
fn http_and_json_error_helpers_format_messages() {
    let http = http_error_for_tests("dial refused");
    assert!(http.to_string().contains("HTTP error"), "{http}");
    let json = json_parse_error_for_tests("eof");
    assert!(json.to_string().contains("JSON parse error"), "{json}");
}
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use tokio_stream::StreamExt;
use whycodes_core::types::{
    ContentBlock, LlmRequest, Message, MessageContent, Role, StreamEvent, ToolDefinition,
};

fn req() -> LlmRequest {
    LlmRequest {
        system: "sys".into(),
        messages: std::sync::Arc::from(vec![Message {
            role: Role::User,
            content: MessageContent::Text("hi".into()),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]),
        tools: std::sync::Arc::from([]),
        max_tokens: Some(16),
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: false,
    }
}

/// Like [`serve_once`], also returning the raw request the server read.
fn serve_once_capture(
    status: &str,
    body: &str,
    content_type: &str,
) -> (String, std::sync::mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let payload = format!("{header}{body}");
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).unwrap_or(0);
            let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
            let _ = stream.write_all(payload.as_bytes());
        }
    });
    (format!("http://{addr}"), rx)
}

fn serve_once(status: &str, body: &str, content_type: &str) -> String {
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
    format!("http://{addr}")
}

#[test]
fn from_base_blank_keeps_cloud_urls() {
    let cloud = GoogleProvider::from_base(Some("   "));
    assert!(
        cloud
            .build_url("gemini-test")
            .contains("generativelanguage")
    );
    let local = GoogleProvider::from_base(Some("http://127.0.0.1:9/"));
    let stream = local.build_url("m");
    assert_eq!(
        stream,
        "http://127.0.0.1:9/v1beta/models/m:streamGenerateContent?alt=sse"
    );
    let complete = local.build_complete_url("m");
    assert_eq!(
        complete,
        "http://127.0.0.1:9/v1beta/models/m:generateContent"
    );
    assert_eq!(GoogleProvider::default().name(), "google");
}

#[test]
fn build_body_covers_roles_empty_system_temp_and_tools() {
    let mut r = req();
    r.system.clear();
    r.max_tokens = None;
    r.temperature = Some(0.25);
    r.messages = std::sync::Arc::from(vec![
        Message {
            role: Role::Assistant,
            content: MessageContent::Text("prev".into()),
            tool_call_id: None,
            name: None,
            created_at: None,
        },
        Message {
            role: Role::Tool,
            content: MessageContent::Text("tool-out".into()),
            tool_call_id: Some("t1".into()),
            name: Some("read".into()),
            created_at: None,
        },
    ]);
    r.tools = vec![ToolDefinition {
        name: "read".into(),
        description: "read a file".into(),
        parameters: serde_json::json!({"type": "object"}),
    }]
    .into();
    let body = GoogleProvider::new().build_body(&r);
    assert!(body.get("systemInstruction").is_none(), "{body}");
    assert_eq!(body["contents"][0]["role"], "model");
    assert_eq!(body["contents"][1]["role"], "user");
    assert!(body["generationConfig"]["temperature"].is_number());
    assert!(body["tools"].is_array(), "{body}");
}

#[tokio::test]
async fn complete_json_parse_error_and_unknown_api_error() {
    let p = GoogleProvider::from_base(Some(&serve_once("200 OK", "not-json", "text/plain")));
    let err = p.complete(&req(), "k", "gemini-test").await.unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("json") || !err.to_string().is_empty(),
        "{err}"
    );

    let p = GoogleProvider::from_base(Some(&serve_once(
        "400 Bad Request",
        "{}",
        "application/json",
    )));
    let err = p.complete(&req(), "k", "gemini-test").await.unwrap_err();
    assert!(
        err.to_string().contains("Unknown") || err.to_string().contains("400"),
        "{err}"
    );
}

fn sse(events: &[serde_json::Value]) -> String {
    events
        .iter()
        .map(|e| format!("data: {e}\r\n\r\n"))
        .collect()
}

async fn collect(mut stream: crate::provider::ProviderEventStream) -> Vec<StreamEvent> {
    let mut out = Vec::new();
    while let Some(ev) = stream.next().await {
        out.push(ev.unwrap());
    }
    out
}

fn text_of(events: &[StreamEvent]) -> String {
    events
        .iter()
        .filter_map(|ev| match ev {
            StreamEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn stream_http_error_surfaces_the_body() {
    let p = GoogleProvider::from_base(Some(&serve_once("403 Forbidden", "denied", "text/plain")));
    let err = p
        .stream(&req(), "k", "gemini-test")
        .await
        .map(|_| ())
        .unwrap_err();
    assert!(err.to_string().contains("denied"), "{err}");
}

/// Regression: `alt=sse` answers `data: {...}` events, but the reply was
/// parsed as a JSON array, so every API-key Gemini turn came back empty.
#[tokio::test]
async fn stream_reads_sse_events_with_the_key_in_a_header() {
    let body = sse(&[
        serde_json::json!({"candidates": [{"content": {"parts": [{"text": "hel"}]}}]}),
        serde_json::json!({
            "candidates": [{"content": {"parts": [{"text": "lo"}]}, "finishReason": "STOP"}],
            "usageMetadata": {"promptTokenCount": 2, "candidatesTokenCount": 3}
        }),
    ]);
    let (base, request) = serve_once_capture("200 OK", &body, "text/event-stream");
    let p = GoogleProvider::from_base(Some(&base));
    let events = collect(p.stream(&req(), "secret-key", "gemini-test").await.unwrap()).await;
    assert_eq!(text_of(&events), "hello");
    assert!(events.iter().any(|e| matches!(
        e,
        StreamEvent::Usage {
            input_tokens: 2,
            output_tokens: 3
        }
    )));
    assert!(matches!(events.last(), Some(StreamEvent::MessageStop)));

    let request = request.recv().unwrap().to_ascii_lowercase();
    let first_line = request.lines().next().unwrap_or_default().to_string();
    assert!(
        first_line.contains(":streamgeneratecontent?alt=sse "),
        "{first_line}"
    );
    assert!(!first_line.contains("secret-key"), "{first_line}");
    assert!(request.contains("x-goog-api-key: secret-key"), "{request}");
}

#[tokio::test]
async fn stream_turns_function_calls_into_tool_use() {
    let body = sse(&[serde_json::json!({
        "candidates": [{"content": {"parts": [
            {"functionCall": {"name": "read", "args": {"path": "a.rs"}}}
        ]}}],
        "usageMetadata": {"promptTokenCount": 1, "candidatesTokenCount": 1}
    })]);
    let p = GoogleProvider::from_base(Some(&serve_once("200 OK", &body, "text/event-stream")));
    let events = collect(p.stream(&req(), "k", "gemini-test").await.unwrap()).await;
    assert!(
        events.iter().any(|e| matches!(
            e,
            StreamEvent::ToolUse { name, input, .. } if name == "read" && input["path"] == "a.rs"
        )),
        "{events:?}"
    );
}

#[tokio::test]
async fn complete_sends_the_key_in_a_header() {
    let json = serde_json::json!({
        "candidates": [{"content": {"parts": [{"text": "ok"}]}}],
        "usageMetadata": {"promptTokenCount": 1, "candidatesTokenCount": 1}
    })
    .to_string();
    let (base, request) = serve_once_capture("200 OK", &json, "application/json");
    let p = GoogleProvider::from_base(Some(&base));
    p.complete(&req(), "secret-key", "gemini-test")
        .await
        .unwrap();
    let request = request.recv().unwrap().to_ascii_lowercase();
    let first_line = request.lines().next().unwrap_or_default().to_string();
    assert!(!first_line.contains("secret-key"), "{first_line}");
    assert!(request.contains("x-goog-api-key: secret-key"), "{request}");
}

#[tokio::test]
async fn complete_candidates_without_parts_are_empty() {
    let json = serde_json::json!({
        "candidates": [{"content": {}}],
        "usageMetadata": {"promptTokenCount": 1, "candidatesTokenCount": 0}
    })
    .to_string();
    let p = GoogleProvider::from_base(Some(&serve_once("200 OK", &json, "application/json")));
    let resp = p.complete(&req(), "k", "gemini-test").await.unwrap();
    assert!(resp.content.is_empty(), "{resp:?}");

    let json = serde_json::json!({
        "candidates": [
            {"content": {"parts": [{"text": "one"}]}},
            {"content": {"parts": [{"text": "two"}]}}
        ],
        "usageMetadata": {"promptTokenCount": 1, "candidatesTokenCount": 2}
    })
    .to_string();
    let p = GoogleProvider::from_base(Some(&serve_once("200 OK", &json, "application/json")));
    let resp = p.complete(&req(), "k", "gemini-test").await.unwrap();
    assert_eq!(resp.content.len(), 2, "{resp:?}");
}

#[tokio::test]
async fn oauth_token_diverts_to_codeassist() {
    use crate::providers::codeassist::{TEST_GEMINI_BASES, cache_project_for_tests};

    fn leak_url(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }
    fn leak_bases(url: &'static str) -> &'static [&'static str] {
        Box::leak(Box::new([url]))
    }

    cache_project_for_tests("google", "proj-google");
    let json = serde_json::json!({
        "candidates": [{"content": {"parts": [{"text": "hello-oauth"}]}}],
        "usageMetadata": {"promptTokenCount": 1, "candidatesTokenCount": 1}
    })
    .to_string();
    let url = leak_url(format!(
        "{}/v1internal",
        serve_once("200 OK", &json, "application/json")
    ));
    TEST_GEMINI_BASES.with(|c| *c.borrow_mut() = Some(leak_bases(url)));
    let resp = GoogleProvider::new()
        .complete(&req(), "ya29.oauth", "gemini-test")
        .await
        .unwrap();
    assert!(
        resp.content.iter().any(|b| matches!(
            b,
            ContentBlock::Text { text } if text.contains("hello-oauth")
        )),
        "{resp:?}"
    );
    TEST_GEMINI_BASES.with(|c| *c.borrow_mut() = None);

    let sse = format!(
        "data: {}\n\n",
        serde_json::json!({"candidates":[{"content":{"parts":[{"text":"hi"}]}}]})
    );
    let url = leak_url(format!(
        "{}/v1internal",
        serve_once("200 OK", &sse, "text/event-stream")
    ));
    TEST_GEMINI_BASES.with(|c| *c.borrow_mut() = Some(leak_bases(url)));
    let mut stream = GoogleProvider::default()
        .stream(&req(), "ya29.oauth", "gemini-test")
        .await
        .unwrap();
    let mut text = String::new();
    while let Some(ev) = stream.next().await {
        if let Ok(StreamEvent::TextDelta { text: d }) = ev {
            text.push_str(&d);
        }
    }
    assert_eq!(text, "hi");
    TEST_GEMINI_BASES.with(|c| *c.borrow_mut() = None);
}
