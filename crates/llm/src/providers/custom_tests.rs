use super::*;

fn make_req() -> LlmRequest {
    LlmRequest {
        system: "You are helpful.".to_string(),
        messages: std::sync::Arc::from([]),
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
fn normalizes_v1_base_to_chat_completions() {
    assert_eq!(
        normalize_chat_completions_url("http://example.local:1234/v1"),
        "http://example.local:1234/v1/chat/completions"
    );
    assert_eq!(
        normalize_chat_completions_url("http://example.local:1234/v1/"),
        "http://example.local:1234/v1/chat/completions"
    );
    assert_eq!(
        normalize_chat_completions_url("http://example.local:1234/v1/chat/completions"),
        "http://example.local:1234/v1/chat/completions"
    );
}

#[test]
fn from_config_uses_normalized_base_url() {
    let pc = whycodes_core::types::ProviderConfig {
        name: "custom".into(),
        api_key: Some("sk-test".into()),
        api_base: None,
        base_url: Some("http://example.local:1234/v1".into()),
        headers: None,
        models: vec!["some/model".into()],
        tool_arguments: None,
        extra: Default::default(),
    };
    let p = CustomProvider::from_config(&pc);
    assert_eq!(
        p.default_base_url(),
        "http://example.local:1234/v1/chat/completions"
    );
    assert_eq!(p.name(), "custom");
    assert_eq!(p.tool_arguments, ToolArgumentsFormat::JsonString);
}

#[test]
fn from_config_honors_tool_arguments_object() {
    let pc = whycodes_core::types::ProviderConfig {
        name: "omniroute".into(),
        api_key: Some("sk-test".into()),
        api_base: None,
        base_url: Some("http://127.0.0.1:9999/v1".into()),
        headers: None,
        models: vec![],
        tool_arguments: Some(ToolArgumentsFormat::Object),
        extra: Default::default(),
    };
    let p = CustomProvider::from_config(&pc);
    assert_eq!(p.tool_arguments, ToolArgumentsFormat::Object);

    let req = make_req();
    let mut req = req;
    // Build a body that includes a tool call in history.
    use whycodes_core::types::{ContentBlock, Message, MessageContent, Role};
    req.messages = std::sync::Arc::from(vec![Message {
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
    let body = p.build_body(&req, "any/model");
    let args = &body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "assistant")
        .unwrap()["tool_calls"][0]["function"]["arguments"];
    assert!(args.is_object(), "provider config asked for object: {args}");
    assert_eq!(args["query"], "nuxt");
}

#[test]
fn test_custom_provider_creation() {
    let p = CustomProvider::new(
        "my-api",
        "https://api.example.com/v1/chat/completions",
        Some("sk-test".to_string()),
        HashMap::from([("X-Custom".to_string(), "val".to_string())]),
    );
    assert_eq!(p.name(), "my-api");
    assert_eq!(
        p.default_base_url(),
        "https://api.example.com/v1/chat/completions"
    );
}

#[test]
fn test_custom_provider_build_body() {
    let p = CustomProvider::new("test", "http://localhost/v1", None, HashMap::new());
    let body = p.build_body(&make_req(), "test-model");
    assert_eq!(body["model"], "test-model");
    assert!(body["messages"].as_array().unwrap()[0]["content"] == "You are helpful.");
}

#[test]
fn test_custom_provider_with_tools() {
    let p = CustomProvider::new("test", "http://localhost/v1", None, HashMap::new());
    let mut req = make_req();
    req.tools = vec![whycodes_core::types::ToolDefinition {
        name: "read".to_string(),
        description: "read file".to_string(),
        parameters: serde_json::json!({"type": "object"}),
    }]
    .into();
    let body = p.build_body(&req, "m");
    assert!(body["tools"].as_array().unwrap().len() == 1);
    assert_eq!(body["tool_choice"], "auto");
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
    format!("http://{addr}/v1")
}

#[tokio::test]
async fn complete_sends_custom_headers_and_falls_back_to_bearer() {
    use crate::provider::LlmProvider;
    use whycodes_core::types::ContentBlock;

    let body = serde_json::json!({
        "choices": [{
            "message": {"role": "assistant", "content": "hello-headers"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1}
    })
    .to_string();
    let url = normalize_chat_completions_url(&serve_once("200 OK", &body, "application/json"));
    let p = CustomProvider::new(
        "custom",
        url,
        Some("sk-test".into()),
        HashMap::from([("X-Custom".into(), "val".into())]),
    );
    let resp = p.complete(&make_req(), "", "m").await.unwrap();
    assert!(
        resp.content.iter().any(|b| matches!(
            b,
            ContentBlock::Text { text } if text.contains("hello-headers")
        )),
        "{resp:?}"
    );

    let err_url =
        normalize_chat_completions_url(&serve_once("400 Bad Request", "{}", "application/json"));
    let p = CustomProvider::new("custom", err_url, None, HashMap::new());
    let err = p.complete(&make_req(), "", "m").await.unwrap_err();
    assert!(
        err.to_string().contains("400") || err.to_string().contains("unknown"),
        "{err}"
    );
}

#[tokio::test]
async fn stream_http_error_includes_status() {
    use crate::provider::LlmProvider;
    let url = normalize_chat_completions_url(&serve_once("502 Bad Gateway", "nope", "text/plain"));
    let p = CustomProvider::new("custom", url, Some("sk".into()), HashMap::new());
    let err = p
        .stream(&make_req(), "", "m")
        .await
        .map(|_| ())
        .unwrap_err();
    assert!(
        err.to_string().contains("502") || err.to_string().contains("nope"),
        "{err}"
    );
}

#[test]
fn from_config_uses_api_base_and_existing_authorization() {
    let pc = whycodes_core::types::ProviderConfig {
        name: "gw".into(),
        api_key: Some("sk-ignored".into()),
        api_base: Some("http://gateway.example/v1".into()),
        base_url: None,
        headers: Some(HashMap::from([(
            "Authorization".into(),
            "Bearer already".into(),
        )])),
        models: vec![],
        tool_arguments: None,
        extra: Default::default(),
    };
    let p = CustomProvider::from_config(&pc);
    assert!(p.default_base_url().ends_with("/chat/completions"));
    assert_eq!(
        p.headers.get("Authorization").map(String::as_str),
        Some("Bearer already")
    );
}
