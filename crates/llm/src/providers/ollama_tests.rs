use super::*;
use whycodes_core::types::{ImageSource, Message, MessageContent, Role};

#[test]
fn normalizes_host_port_and_v1_roots() {
    assert_eq!(
        normalize_ollama_chat_url_with_env(None, None),
        "http://localhost:11434/api/chat"
    );
    assert_eq!(
        normalize_ollama_chat_url_with_env(Some("http://127.0.0.1:4554"), None),
        "http://127.0.0.1:4554/api/chat"
    );
    assert_eq!(
        normalize_ollama_chat_url_with_env(Some("127.0.0.1:4554"), None),
        "http://127.0.0.1:4554/api/chat"
    );
    assert_eq!(
        normalize_ollama_chat_url_with_env(Some("http://127.0.0.1:4554/"), None),
        "http://127.0.0.1:4554/api/chat"
    );
    assert_eq!(
        normalize_ollama_chat_url_with_env(Some("http://127.0.0.1:4554/api/chat"), None),
        "http://127.0.0.1:4554/api/chat"
    );
    assert_eq!(
        normalize_ollama_chat_url_with_env(Some("http://127.0.0.1:4554/v1"), None),
        "http://127.0.0.1:4554/api/chat"
    );
    assert_eq!(
        normalize_ollama_chat_url_with_env(Some("http://127.0.0.1:4554/v1/chat/completions"), None),
        "http://127.0.0.1:4554/api/chat"
    );
    assert_eq!(
        normalize_ollama_chat_url_with_env(None, Some("127.0.0.1:4554")),
        "http://127.0.0.1:4554/api/chat"
    );
    // Explicit config wins over OLLAMA_HOST.
    assert_eq!(
        normalize_ollama_chat_url_with_env(Some("http://127.0.0.1:4554"), Some("10.0.0.1:1")),
        "http://127.0.0.1:4554/api/chat"
    );
}

fn ollama_host_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn ollama_host_env_is_read_when_base_is_none() {
    let _guard = ollama_host_lock();
    let prev = std::env::var("OLLAMA_HOST").ok();
    unsafe {
        std::env::remove_var("OLLAMA_HOST");
    }
    match None::<String> {
        Some(v) => unsafe { std::env::set_var("OLLAMA_HOST", v) },
        None => unsafe { std::env::remove_var("OLLAMA_HOST") },
    }
    unsafe {
        std::env::set_var("OLLAMA_HOST", "127.0.0.1:4554");
    }
    assert_eq!(
        normalize_ollama_chat_url(None),
        "http://127.0.0.1:4554/api/chat"
    );
    match prev {
        Some(v) => unsafe { std::env::set_var("OLLAMA_HOST", v) },
        None => unsafe { std::env::remove_var("OLLAMA_HOST") },
    }
}

#[cfg(unix)]
#[test]
fn ollama_host_not_unicode_falls_back_to_default() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let _guard = ollama_host_lock();
    let prev = std::env::var_os("OLLAMA_HOST");
    unsafe {
        std::env::remove_var("OLLAMA_HOST");
    }
    match None::<std::ffi::OsString> {
        Some(v) => unsafe { std::env::set_var("OLLAMA_HOST", v) },
        None => unsafe { std::env::remove_var("OLLAMA_HOST") },
    }
    unsafe {
        std::env::set_var("OLLAMA_HOST", OsString::from_vec(vec![0xff, 0xfe]));
    }
    assert_eq!(
        normalize_ollama_chat_url(None),
        "http://localhost:11434/api/chat"
    );
    match prev {
        Some(v) => unsafe { std::env::set_var("OLLAMA_HOST", v) },
        None => unsafe { std::env::remove_var("OLLAMA_HOST") },
    }
}

#[test]
fn from_config_uses_base_url() {
    let pc = whycodes_core::types::ProviderConfig {
        name: "ollama".into(),
        api_key: None,
        api_base: None,
        base_url: Some("http://127.0.0.1:4554".into()),
        headers: None,
        models: vec![],
        tool_arguments: None,
        extra: Default::default(),
    };
    let p = OllamaProvider::from_config(&pc);
    assert_eq!(p.default_base_url(), "http://127.0.0.1:4554/api/chat");
}

#[test]
fn done_chunk_emits_usage() {
    let event = serde_json::json!({
        "message": {"content": ""},
        "done": true,
        "prompt_eval_count": 12,
        "eval_count": 4,
    });
    let (events, done) = events_from_ollama_object(&event);
    assert!(done);
    assert!(events.iter().any(|e| matches!(
        e,
        StreamEvent::Usage {
            input_tokens: 12,
            output_tokens: 4
        }
    )));
}

#[test]
fn text_delta_before_done() {
    let event = serde_json::json!({
        "message": {"content": "hi"},
        "done": false,
    });
    let (events, done) = events_from_ollama_object(&event);
    assert!(!done);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::TextDelta { text } if text == "hi"))
    );
}

#[test]
fn convert_messages_covers_roles_and_base64_images() {
    let req = LlmRequest {
        system: "sys".into(),
        messages: std::sync::Arc::from(vec![
            Message {
                role: Role::System,
                content: MessageContent::Text("inner".into()),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
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
                tool_call_id: Some("c1".into()),
                name: Some("read".into()),
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
                ]),
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
    let msgs = OllamaProvider::new().convert_messages(&req);
    let roles: Vec<&str> = msgs.iter().filter_map(|m| m["role"].as_str()).collect();
    assert_eq!(roles, ["system", "system", "assistant", "tool", "user"]);
    let user = msgs.iter().find(|m| m["role"] == "user").unwrap();
    assert_eq!(user["images"][0], "AAAA");
    assert!(user.get("images").unwrap().as_array().unwrap().len() == 1);
}

#[test]
fn events_from_object_map_tool_calls_object_and_string() {
    let event = serde_json::json!({
        "message": {
            "content": "",
            "tool_calls": [
                {"id": "c1", "function": {"name": "read", "arguments": {"p": 1}}},
                {"function": {"name": "write", "arguments": "{\"q\":2}"}}
            ]
        },
        "done": false
    });
    let (events, done) = events_from_ollama_object(&event);
    assert!(!done);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::ToolUse { name, .. } if name == "read"))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::ToolUse { name, .. } if name == "write"))
    );
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
    format!("http://{addr}")
}

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
async fn complete_parses_tool_calls_and_bearer_auth() {
    let body = serde_json::json!({
        "message": {
            "content": "ok",
            "tool_calls": [{
                "id": "c1",
                "function": {"name": "read", "arguments": "{\"p\":1}"}
            }]
        },
        "done": true,
        "prompt_eval_count": 2,
        "eval_count": 3
    })
    .to_string();
    let p = OllamaProvider::from_base(Some(&serve_once("200 OK", &body, "application/json")));
    let resp = p.complete(&req(), "secret-key", "llama").await.unwrap();
    assert!(
        resp.content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolUse { name, .. } if name == "read"))
    );
    assert_eq!(resp.stop_reason.as_deref(), Some("stop"));
    assert_eq!(resp.usage.input_tokens, 2);
}

#[tokio::test]
async fn stream_http_error_and_inline_error_object() {
    let p = OllamaProvider::from_base(Some(&serve_once(
        "500 Internal Server Error",
        "boom",
        "text/plain",
    )));
    let err = p.stream(&req(), "", "llama").await.map(|_| ()).unwrap_err();
    assert!(
        err.to_string().contains("boom") || !err.to_string().is_empty(),
        "{err}"
    );

    let ndjson = format!("{}\n", serde_json::json!({"error": "inline"}));
    let p = OllamaProvider::from_base(Some(&serve_once("200 OK", &ndjson, "application/json")));
    let mut stream = p.stream(&req(), "", "llama").await.unwrap();
    use tokio_stream::StreamExt;
    let mut saw_err = false;
    while let Some(ev) = stream.next().await {
        if ev.is_err() {
            saw_err = true;
        }
    }
    assert!(saw_err);
}

#[tokio::test]
async fn stream_emits_tool_use_and_message_stop_without_done() {
    let ndjson = format!(
        "{}\n",
        serde_json::json!({
            "message": {
                "content": "hi",
                "tool_calls": [{"id": "c1", "function": {"name": "read", "arguments": {"p": 1}}}]
            }
        })
    );
    let p = OllamaProvider::from_base(Some(&serve_once("200 OK", &ndjson, "application/x-ndjson")));
    let mut stream = p.stream(&req(), "k", "llama").await.unwrap();
    use tokio_stream::StreamExt;
    let mut text = String::new();
    let mut saw_tool = false;
    let mut saw_stop = false;
    while let Some(ev) = stream.next().await {
        match ev.unwrap() {
            StreamEvent::TextDelta { text: d } => text.push_str(&d),
            StreamEvent::ToolUse { name, .. } if name == "read" => saw_tool = true,
            StreamEvent::MessageStop => saw_stop = true,
            _ => {}
        }
    }
    assert_eq!(text, "hi");
    assert!(saw_tool);
    assert!(saw_stop);
}

#[tokio::test]
async fn stream_finishes_leftover_line_and_skips_noise() {
    let leftover = serde_json::json!({
        "message": {"content": "tail"},
        "done": true,
        "prompt_eval_count": 1,
        "eval_count": 1
    })
    .to_string();
    let p = OllamaProvider::from_base(Some(&serve_once(
        "200 OK",
        &leftover,
        "application/x-ndjson",
    )));
    let mut stream = p.stream(&req(), "", "llama").await.unwrap();
    use tokio_stream::StreamExt;
    let mut text = String::new();
    let mut saw_stop = false;
    while let Some(ev) = stream.next().await {
        match ev.unwrap() {
            StreamEvent::TextDelta { text: d } => text.push_str(&d),
            StreamEvent::MessageStop => saw_stop = true,
            _ => {}
        }
    }
    assert_eq!(text, "tail");
    assert!(saw_stop);

    let noisy = format!(
        "\nnot-json\n{}\n",
        serde_json::json!({"message": {"content": "ok"}, "done": true})
    );
    let p = OllamaProvider::from_base(Some(&serve_once("200 OK", &noisy, "application/x-ndjson")));
    let mut stream = p.stream(&req(), "", "llama").await.unwrap();
    let mut text = String::new();
    while let Some(ev) = stream.next().await {
        if let Ok(StreamEvent::TextDelta { text: d }) = ev {
            text.push_str(&d);
        }
    }
    assert_eq!(text, "ok");
}

#[test]
fn events_from_object_without_message_are_empty() {
    let event = serde_json::json!({"done": false});
    let (events, done) = events_from_ollama_object(&event);
    assert!(!done);
    assert!(events.is_empty());
    let event = serde_json::json!({
        "message": {
            "content": "ok",
            "tool_calls": [
                {"function": {"name": "a", "arguments": null}},
                {"function": {"name": "b", "arguments": {}}}
            ]
        },
        "done": false
    });
    let (events, _) = events_from_ollama_object(&event);
    assert_eq!(events.len(), 3);
}
