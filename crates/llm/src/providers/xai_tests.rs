use super::*;
use whycodes_core::types::StreamEvent;

#[test]
fn oauth_tokens_are_distinguished_from_console_keys() {
    assert!(is_xai_oauth_token("eyJhbGciOiJ.eyJzdWIiOiJx.sig"));
    assert!(is_xai_oauth_token("opaque-oauth-token"));
    assert!(!is_xai_oauth_token("xai-abc123"));
    assert!(!is_xai_oauth_token(""));
    assert_eq!(
        inference_url("eyJhbGciOiJ.eyJzdWIiOiJx.sig"),
        SUBSCRIPTION_CHAT_URL
    );
    assert_eq!(inference_url("xai-abc123"), CONSOLE_CHAT_URL);
}

#[test]
fn from_base_blank_falls_back_to_console() {
    let blank = XaiProvider::from_base(Some("   "));
    assert_eq!(blank.default_base_url(), CONSOLE_CHAT_URL);
    let local = XaiProvider::from_base(Some("http://127.0.0.1:9/v1"));
    let local_url = local.default_base_url();
    assert!(local_url.ends_with("/chat/completions"), "{local_url}");
    assert_eq!(XaiProvider::default().name(), "xai");
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
    format!("http://{addr}/v1/chat/completions")
}

fn req() -> LlmRequest {
    LlmRequest {
        system: "sys".into(),
        messages: std::sync::Arc::from(vec![whycodes_core::types::Message {
            role: whycodes_core::types::Role::User,
            content: whycodes_core::types::MessageContent::Text("hi".into()),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]),
        tools: std::sync::Arc::from([]),
        max_tokens: Some(8),
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: false,
    }
}

#[tokio::test]
async fn oauth_token_uses_subscription_url_and_plugin_identity() {
    let body = serde_json::json!({
        "choices": [{
            "message": {"role": "assistant", "content": "hello-xai"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 1, "completion_tokens": 2}
    })
    .to_string();
    let url: &'static str =
        Box::leak(serve_once("200 OK", &body, "application/json").into_boxed_str());
    TEST_SUBSCRIPTION_URL.with(|c| *c.borrow_mut() = Some(url));
    let resp = XaiProvider::from_base(None)
        .complete(&req(), "opaque-oauth-token", "grok")
        .await
        .unwrap();
    assert!(
        resp.content.iter().any(|b| matches!(
            b,
            whycodes_core::types::ContentBlock::Text { text } if text.contains("hello-xai")
        )),
        "{resp:?}"
    );
    TEST_SUBSCRIPTION_URL.with(|c| *c.borrow_mut() = None);

    let sse = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
        "data: [DONE]\n\n",
    );
    let url: &'static str =
        Box::leak(serve_once("200 OK", sse, "text/event-stream").into_boxed_str());
    TEST_SUBSCRIPTION_URL.with(|c| *c.borrow_mut() = Some(url));
    let mut stream = XaiProvider::from_base(None)
        .stream(&req(), "opaque-oauth-token", "grok")
        .await
        .unwrap();
    use tokio_stream::StreamExt;
    let mut text = String::new();
    while let Some(ev) = stream.next().await {
        if let Ok(StreamEvent::TextDelta { text: d }) = ev {
            text.push_str(&d);
        }
    }
    assert_eq!(text, "hi");
    TEST_SUBSCRIPTION_URL.with(|c| *c.borrow_mut() = None);
    assert_eq!(inference_url("opaque-oauth-token"), SUBSCRIPTION_CHAT_URL);
}
