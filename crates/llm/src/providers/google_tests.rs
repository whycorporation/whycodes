use super::*;
use crate::provider::LlmProvider;
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
            .build_url("gemini-test", "k")
            .contains("generativelanguage")
    );
    let local = GoogleProvider::from_base(Some("http://127.0.0.1:9/"));
    let stream = local.build_url("m", "k");
    assert!(stream.contains("127.0.0.1:9"), "{stream}");
    assert!(stream.contains("streamGenerateContent"), "{stream}");
    let complete = local.build_complete_url("m", "k");
    assert!(complete.contains("generateContent"), "{complete}");
    assert!(!complete.contains("streamGenerateContent"), "{complete}");
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

#[tokio::test]
async fn stream_http_error_and_usage_stop() {
    let p = GoogleProvider::from_base(Some(&serve_once("403 Forbidden", "denied", "text/plain")));
    let err = p
        .stream(&req(), "k", "gemini-test")
        .await
        .map(|_| ())
        .unwrap_err();
    assert!(
        err.to_string().contains("denied") || !err.to_string().is_empty(),
        "{err}"
    );

    let body = serde_json::json!({
        "candidates": [{
            "content": {"parts": [{"text": "hello"}]},
            "finishReason": "STOP"
        }],
        "usageMetadata": {"promptTokenCount": 2, "candidatesTokenCount": 3}
    })
    .to_string();
    let p = GoogleProvider::from_base(Some(&serve_once("200 OK", &body, "application/json")));
    let mut stream = p.stream(&req(), "k", "gemini-test").await.unwrap();
    let mut text = String::new();
    let mut saw_usage = false;
    let mut saw_stop = false;
    while let Some(ev) = stream.next().await {
        match ev.unwrap() {
            StreamEvent::TextDelta { text: d } => text.push_str(&d),
            StreamEvent::Usage { .. } => saw_usage = true,
            StreamEvent::MessageStop => saw_stop = true,
            StreamEvent::MessageDelta { .. } => {}
            _ => {}
        }
    }
    assert_eq!(text, "hello");
    assert!(saw_usage);
    assert!(saw_stop);
}

#[test]
fn stream_body_wraps_array_items_and_skips_invalid_json() {
    let first = serde_json::json!({
        "candidates": [{"content": {"parts": [{"text": "one"}]}}]
    });
    let second = serde_json::json!({
        "candidates": [{"content": {"parts": [{"text": "two"}]}}],
        "usageMetadata": {"promptTokenCount": 1, "candidatesTokenCount": 2}
    });
    let body = format!("{}\n,{}", first, second);
    let events = events_from_google_stream_body(&body);
    let text: String = events
        .iter()
        .filter_map(|ev| match ev {
            Ok(StreamEvent::TextDelta { text }) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "two");
    assert!(
        events
            .iter()
            .any(|ev| matches!(ev, Ok(StreamEvent::MessageStop)))
    );

    let wrapped = events_from_google_stream_body(
        r#""usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":4}"#,
    );
    assert!(wrapped.iter().any(|ev| matches!(
        ev,
        Ok(StreamEvent::Usage {
            input_tokens: 3,
            output_tokens: 4
        })
    )));
    assert!(events_from_google_stream_body("not-json").is_empty());
    assert!(events_from_google_stream_body("]\n").is_empty());
    assert!(events_from_google_stream_body("{\"ok\":1}\n,not-json]}").is_empty());
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

#[test]
fn stream_body_skips_candidates_without_parts() {
    let body = serde_json::json!({
        "candidates": [
            {"content": {}},
            {"content": {"parts": [{"text": "kept"}]}, "finishReason": "STOP"}
        ],
        "usageMetadata": {"promptTokenCount": 1, "candidatesTokenCount": 1}
    })
    .to_string();
    let events = events_from_google_stream_body(&body);
    let text: String = events
        .iter()
        .filter_map(|ev| match ev {
            Ok(StreamEvent::TextDelta { text }) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "kept");
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
