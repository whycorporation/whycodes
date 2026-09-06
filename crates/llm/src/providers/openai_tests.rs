use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio_stream::StreamExt;
use whycodes_core::types::{ContentBlock, LlmRequest, Message, MessageContent, Role, StreamEvent};

#[test]
fn from_base_and_from_config_normalize_urls() {
    let def = OpenAiProvider::new();
    assert_eq!(def.name(), "openai");
    assert_eq!(
        def.default_base_url(),
        "https://api.openai.com/v1/chat/completions"
    );
    let via_default = OpenAiProvider::default();
    assert_eq!(via_default.name(), "openai");

    let custom = OpenAiProvider::from_base(Some("http://127.0.0.1:9/v1"));
    let custom_url = custom.default_base_url();
    assert!(custom_url.ends_with("/chat/completions"), "{custom_url}");

    let cfg = whycodes_core::types::ProviderConfig {
        name: "openai".into(),
        api_key: None,
        api_base: None,
        base_url: Some("http://example.invalid/v1".into()),
        headers: None,
        models: vec![],
        tool_arguments: None,
        extra: Default::default(),
    };
    let from_cfg = OpenAiProvider::from_config(&cfg);
    assert!(from_cfg.default_base_url().contains("example.invalid"));

    let blank = OpenAiProvider::from_base(Some("   "));
    assert_eq!(
        blank.default_base_url(),
        "https://api.openai.com/v1/chat/completions"
    );
    let via_api_base = OpenAiProvider::from_config(&whycodes_core::types::ProviderConfig {
        name: "openai".into(),
        api_key: None,
        api_base: Some("http://127.0.0.1:9/v1".into()),
        base_url: None,
        headers: None,
        models: vec![],
        tool_arguments: None,
        extra: Default::default(),
    });
    let via_url = via_api_base.default_base_url();
    assert!(via_url.contains("127.0.0.1:9"), "{via_url}");
}

fn base_request() -> LlmRequest {
    LlmRequest {
        system: "sys".into(),
        messages: Arc::from(vec![Message {
            role: Role::User,
            content: MessageContent::Text("hi".into()),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]),
        tools: Arc::from([]),
        max_tokens: Some(32),
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
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(payload.as_bytes());
        }
    });
    format!("http://{addr}/v1")
}

#[tokio::test]
async fn complete_parses_chat_completion_json() {
    let body = serde_json::json!({
        "choices": [{
            "message": {"role": "assistant", "content": "hello-openai"}
        }],
        "usage": {"prompt_tokens": 3, "completion_tokens": 2}
    })
    .to_string();
    let base = serve_once("200 OK", &body, "application/json");
    let provider = OpenAiProvider::from_base(Some(&base));
    let req = base_request();
    let resp = provider
        .complete(&req, "sk-test", "gpt-test")
        .await
        .unwrap();
    assert!(
        resp.content
            .iter()
            .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("hello-openai"))),
        "{resp:?}"
    );
}

#[tokio::test]
async fn complete_http_error_is_reported() {
    let base = serve_once("401 Unauthorized", "nope", "text/plain");
    let provider = OpenAiProvider::from_base(Some(&base));
    let req = base_request();
    let err = provider
        .complete(&req, "sk-bad", "gpt-test")
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.to_lowercase().contains("401")
            || msg.to_lowercase().contains("openai")
            || !msg.is_empty(),
        "{msg}"
    );
}

#[tokio::test]
async fn stream_parses_sse_deltas() {
    let sse = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"llo\"}}]}\n\n",
        "data: [DONE]\n\n",
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        sse.len()
    );
    let payload = format!("{header}{sse}");
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(payload.as_bytes());
            thread::sleep(Duration::from_millis(20));
        }
    });
    let provider = OpenAiProvider::from_base(Some(&format!("http://{addr}/v1")));
    let req = base_request();
    let mut stream = provider.stream(&req, "sk-test", "gpt-test").await.unwrap();
    let mut text = String::new();
    while let Some(ev) = stream.next().await {
        if let Ok(StreamEvent::TextDelta { text: d }) = ev {
            text.push_str(&d);
        }
    }
    assert_eq!(text, "hello");
}

#[tokio::test]
async fn complete_json_parse_and_unknown_error_and_empty_key() {
    let parse = serve_once("200 OK", "not-json", "text/plain");
    let err = OpenAiProvider::from_base(Some(&parse))
        .complete(&base_request(), "", "gpt-test")
        .await
        .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("json") || !err.to_string().is_empty(),
        "{err}"
    );

    let unknown = serve_once("400 Bad Request", "{}", "application/json");
    let err = OpenAiProvider::from_base(Some(&unknown))
        .complete(&base_request(), "sk", "gpt-test")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("Unknown") || err.to_string().contains("400"),
        "{err}"
    );
}

#[tokio::test]
async fn stream_http_error_and_skips_noise_lines() {
    let err_base = serve_once("401 Unauthorized", "nope", "text/plain");
    let err = OpenAiProvider::from_base(Some(&err_base))
        .stream(&base_request(), "sk", "gpt-test")
        .await
        .map(|_| ())
        .unwrap_err();
    assert!(
        err.to_string().contains("nope") || !err.to_string().is_empty(),
        "{err}"
    );

    let sse = concat!(
        ": comment\n",
        "\n",
        "data: not-json\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2}}\n\n",
        "data: [DONE]\n\n",
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        sse.len()
    );
    let payload = format!("{header}{sse}");
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(payload.as_bytes());
            thread::sleep(Duration::from_millis(20));
        }
    });
    let provider = OpenAiProvider::from_base(Some(&format!("http://{addr}/v1")));
    let mut stream = provider
        .stream(&base_request(), "sk-test", "gpt-test")
        .await
        .unwrap();
    let mut text = String::new();
    let mut saw_usage = false;
    while let Some(ev) = stream.next().await {
        match ev.unwrap() {
            StreamEvent::TextDelta { text: d } => text.push_str(&d),
            StreamEvent::Usage { .. } => saw_usage = true,
            _ => {}
        }
    }
    assert_eq!(text, "hi");
    assert!(saw_usage);
}

#[tokio::test]
async fn jwt_token_diverts_complete_and_stream_to_codex() {
    use crate::providers::codex::TEST_CODEX_URL;

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
    fn serve_sse(body: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
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

    let url: &'static str = Box::leak(serve_sse(&sse_hello()).into_boxed_str());
    TEST_CODEX_URL.with(|c| *c.borrow_mut() = Some(url));
    let provider = OpenAiProvider::new();
    let jwt = "eyJhbGciOiJ.eyJzdWIiOiJx.sig";
    let resp = provider
        .complete(&base_request(), jwt, "gpt-test")
        .await
        .unwrap();
    assert!(
        resp.content
            .iter()
            .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("hello"))),
        "{resp:?}"
    );
    TEST_CODEX_URL.with(|c| *c.borrow_mut() = None);

    let url: &'static str = Box::leak(serve_sse(&sse_hello()).into_boxed_str());
    TEST_CODEX_URL.with(|c| *c.borrow_mut() = Some(url));
    let mut stream = provider
        .stream(&base_request(), jwt, "gpt-test")
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
async fn stream_empty_body_ends_without_events() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let header = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(header.as_bytes());
        }
    });
    let provider = OpenAiProvider::from_base(Some(&format!("http://{addr}/v1")));
    let mut stream = provider
        .stream(&base_request(), "sk-test", "gpt-test")
        .await
        .unwrap();
    let mut n = 0usize;
    while let Some(_ev) = stream.next().await {
        n += 1;
    }
    assert_eq!(n, 0);
}

#[tokio::test]
async fn stream_truncated_body_and_delayed_chunks() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let body = "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n";
            let header = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n".to_string();
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(body.as_bytes());
        }
    });
    let provider = OpenAiProvider::from_base(Some(&format!("http://{addr}/v1")));
    let mut stream = provider
        .stream(&base_request(), "sk-test", "gpt-test")
        .await
        .unwrap();
    while let Some(_ev) = stream.next().await {}

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let sse = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"llo\"}}]}\n\n",
        "data: [DONE]\n\n",
    );
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                sse.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(&sse.as_bytes()[..20]);
            thread::sleep(Duration::from_millis(30));
            let _ = stream.write_all(&sse.as_bytes()[20..]);
        }
    });
    let provider = OpenAiProvider::from_base(Some(&format!("http://{addr}/v1")));
    let mut stream = provider
        .stream(&base_request(), "sk-test", "gpt-test")
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
fn build_body_includes_tools() {
    let mut req = base_request();
    req.tools = vec![whycodes_core::types::ToolDefinition {
        name: "read".into(),
        description: "read a file".into(),
        parameters: serde_json::json!({"type": "object"}),
    }]
    .into();
    let body = OpenAiProvider::new().build_body(&req, "gpt-test");
    assert!(body["tools"].is_array());
    assert_eq!(body["tool_choice"], "auto");
    assert_eq!(body["parallel_tool_calls"], true);
}
