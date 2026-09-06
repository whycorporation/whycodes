use super::*;

#[test]
fn identity() {
    let p = AntigravityProvider::new();
    assert_eq!(p.name(), "google-antigravity");
    assert_eq!(
        p.default_base_url(),
        "https://daily-cloudcode-pa.googleapis.com/v1internal"
    );
    let via_default = AntigravityProvider::default();
    assert_eq!(via_default.name(), p.name());
    assert_eq!(via_default.default_base_url(), p.default_base_url());
}

#[tokio::test]
async fn complete_and_stream_use_codeassist_loopback() {
    use tokio_stream::StreamExt;
    use whycodes_core::types::{
        ContentBlock, LlmRequest, Message, MessageContent, Role, StreamEvent,
    };

    fn leak_url(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }
    fn leak_bases(url: &'static str) -> &'static [&'static str] {
        Box::leak(Box::new([url]))
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

    crate::providers::codeassist::cache_project_for_tests("google-antigravity", "proj-anti");
    let json = serde_json::json!({
        "candidates": [{"content": {"parts": [{"text": "hello-anti"}]}}],
        "usageMetadata": {"promptTokenCount": 1, "candidatesTokenCount": 1}
    })
    .to_string();
    let url = leak_url(serve_once("200 OK", &json, "application/json"));
    crate::providers::codeassist::TEST_ANTIGRAVITY_BASES
        .with(|c| *c.borrow_mut() = Some(leak_bases(url)));
    let req = LlmRequest {
        system: String::new(),
        messages: std::sync::Arc::from(vec![Message {
            role: Role::User,
            content: MessageContent::Text("hi".into()),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]),
        tools: std::sync::Arc::from([]),
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: false,
    };
    let resp = AntigravityProvider::new()
        .complete(&req, "ya29.anti", "gemini-3.1-pro")
        .await
        .unwrap();
    assert!(
        resp.content.iter().any(|b| matches!(
            b,
            ContentBlock::Text { text } if text.contains("hello-anti")
        )),
        "{resp:?}"
    );
    crate::providers::codeassist::TEST_ANTIGRAVITY_BASES.with(|c| *c.borrow_mut() = None);

    let sse = format!(
        "data: {}\n\n",
        serde_json::json!({"candidates":[{"content":{"parts":[{"text":"hi"}]}}]})
    );
    let url = leak_url(serve_once("200 OK", &sse, "text/event-stream"));
    crate::providers::codeassist::TEST_ANTIGRAVITY_BASES
        .with(|c| *c.borrow_mut() = Some(leak_bases(url)));
    let mut stream = AntigravityProvider::default()
        .stream(&req, "ya29.anti", "gemini-3.1-pro")
        .await
        .unwrap();
    let mut text = String::new();
    while let Some(ev) = stream.next().await {
        if let Ok(StreamEvent::TextDelta { text: d }) = ev {
            text.push_str(&d);
        }
    }
    assert_eq!(text, "hi");
    crate::providers::codeassist::TEST_ANTIGRAVITY_BASES.with(|c| *c.borrow_mut() = None);
}
