//! Built-in LLM provider implementations.

pub mod anthropic;
pub mod antigravity;
pub mod codeassist;
pub mod codex;
pub mod copilot;
pub mod custom;
pub mod deepseek;
pub mod google;
pub mod groq;
pub mod mistral;
pub mod ollama;
pub mod openai;
pub mod openrouter;
pub mod together;
pub mod xai;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::LlmProvider;
    use whycodes_core::types::{LlmRequest, Message, MessageContent, Role, ToolDefinition};

    fn req_with_tools() -> LlmRequest {
        LlmRequest {
            system: "sys".into(),
            messages: std::sync::Arc::from(vec![Message {
                role: Role::User,
                content: MessageContent::Text("hi".into()),
                tool_call_id: None,
                name: None,
                created_at: None,
            }]),
            tools: vec![ToolDefinition {
                name: "read".into(),
                description: "read a file".into(),
                parameters: serde_json::json!({"type": "object"}),
            }]
            .into(),
            max_tokens: Some(32),
            temperature: Some(0.2),
            top_p: Some(0.9),
            top_k: None,
            stop_sequences: None,
            thinking: None,
            use_prompt_cache: false,
        }
    }

    fn assert_openai_compat_body(body: &serde_json::Value, model: &str) {
        assert_eq!(body["model"], model);
        assert_eq!(body["stream"], true);
        assert_eq!(body["max_tokens"], 32);
        assert!(body["tools"].is_array(), "{body}");
        assert_eq!(body["tool_choice"], "auto");
        assert_eq!(body["parallel_tool_calls"], true);
        assert!(body["temperature"].is_number(), "{body}");
        assert!(body["top_p"].is_number(), "{body}");
        assert!(body["messages"].is_array(), "{body}");
    }

    #[test]
    fn openai_compat_wrappers_build_body_and_name_their_host() {
        let req = req_with_tools();
        let groq = groq::GroqProvider::new();
        let mistral = mistral::MistralProvider::new();
        let together = together::TogetherProvider::new();
        let xai = xai::XaiProvider::new();
        let deepseek = deepseek::DeepSeekProvider::new();
        let cases = [
            (
                groq.name(),
                groq.default_base_url(),
                groq.build_body(&req, "llama"),
                "groq",
                "https://api.groq.com/openai/v1/chat/completions",
                "llama",
            ),
            (
                mistral.name(),
                mistral.default_base_url(),
                mistral.build_body(&req, "mistral-small"),
                "mistral",
                "https://api.mistral.ai/v1/chat/completions",
                "mistral-small",
            ),
            (
                together.name(),
                together.default_base_url(),
                together.build_body(&req, "together-model"),
                "together",
                "https://api.together.xyz/v1/chat/completions",
                "together-model",
            ),
            (
                xai.name(),
                xai.default_base_url(),
                xai.build_body(&req, "grok"),
                "xai",
                "https://api.x.ai/v1/chat/completions",
                "grok",
            ),
            (
                deepseek.name(),
                deepseek.default_base_url(),
                deepseek.build_body(&req, "deepseek-chat"),
                "deepseek",
                "https://api.deepseek.com/v1/chat/completions",
                "deepseek-chat",
            ),
        ];
        for (name, url, body, exp_name, exp_url, model) in cases {
            assert_eq!(name, exp_name);
            assert_eq!(url, exp_url);
            assert_openai_compat_body(&body, model);
        }
    }

    #[test]
    fn ollama_body_uses_options_and_system_message() {
        let p = ollama::OllamaProvider::new();
        assert_eq!(p.name(), "ollama");
        let body = p.build_body(&req_with_tools(), "llama3");
        assert_eq!(body["model"], "llama3");
        assert_eq!(body["options"]["num_predict"], 32);
        assert!(body["options"]["temperature"].is_number());
        assert!(body["options"]["top_p"].is_number());
        assert!(body["tools"].is_array());
        let roles: Vec<&str> = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["role"].as_str())
            .collect();
        assert!(roles.contains(&"system"), "{roles:?}");
        assert!(roles.contains(&"user"), "{roles:?}");
        assert!(
            p.default_base_url().ends_with("/api/chat"),
            "{}",
            p.default_base_url()
        );
    }

    #[test]
    fn google_body_and_urls() {
        let p = google::GoogleProvider::new();
        assert_eq!(p.name(), "google");
        let req = req_with_tools();
        let body = p.build_body(&req);
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "sys");
        assert_eq!(body["contents"][0]["role"], "user");
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 32);
        assert!(body["tools"].is_array());

        let stream = p.build_url("gemini-2.0-flash", "k");
        assert!(stream.contains("streamGenerateContent"), "{stream}");
        assert!(stream.contains("key=k"), "{stream}");
        let complete = p.build_complete_url("gemini-2.0-flash", "k");
        assert!(complete.contains("generateContent"), "{complete}");
        assert!(!complete.contains("streamGenerateContent"), "{complete}");
    }

    #[test]
    fn copilot_identity() {
        let p = copilot::CopilotProvider::new();
        assert_eq!(p.name(), "github-copilot");
        assert_eq!(
            p.default_base_url(),
            "https://api.githubcopilot.com/chat/completions"
        );
    }

    #[test]
    fn antigravity_identity() {
        let p = antigravity::AntigravityProvider::new();
        assert_eq!(p.name(), "google-antigravity");
        assert_eq!(
            p.default_base_url(),
            "https://daily-cloudcode-pa.googleapis.com/v1internal"
        );
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

    fn ok_json() -> String {
        serde_json::json!({
            "choices": [{
                "message": {"role": "assistant", "content": "hello-compat"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 2}
        })
        .to_string()
    }

    async fn assert_complete_ok(provider: &dyn crate::provider::LlmProvider) {
        use whycodes_core::types::ContentBlock;
        let req = req();
        let resp = provider.complete(&req, "sk-test", "m").await.unwrap();
        assert!(
            resp.content
                .iter()
                .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("hello-compat"))),
            "{resp:?}"
        );
    }

    async fn assert_complete_err(provider: &dyn crate::provider::LlmProvider) {
        let req = req();
        let err = provider.complete(&req, "sk-bad", "m").await.unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    async fn assert_stream_hello(provider: &dyn crate::provider::LlmProvider) {
        use tokio_stream::StreamExt;
        use whycodes_core::types::StreamEvent;
        let req = req();
        let mut stream = provider.stream(&req, "sk-test", "m").await.unwrap();
        let mut text = String::new();
        while let Some(ev) = stream.next().await {
            if let Ok(StreamEvent::TextDelta { text: d }) = ev {
                text.push_str(&d);
            }
        }
        assert_eq!(text, "hello");
    }

    fn sse_hello() -> String {
        concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"llo\"}}]}\n\n",
            "data: [DONE]\n\n",
        )
        .to_string()
    }

    fn serve_sse(body: &str) -> String {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;
        use std::time::Duration;
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
        format!("http://{addr}/v1")
    }

    #[tokio::test]
    async fn openai_compat_wrappers_complete_and_stream_against_loopback() {
        let json = ok_json();
        let sse = sse_hello();

        let groq_ok =
            groq::GroqProvider::from_base(Some(&serve_once("200 OK", &json, "application/json")));
        assert_complete_ok(&groq_ok).await;
        let groq_err = groq::GroqProvider::from_base(Some(&serve_once(
            "401 Unauthorized",
            "nope",
            "text/plain",
        )));
        assert_complete_err(&groq_err).await;
        let groq_stream = groq::GroqProvider::from_base(Some(&serve_sse(&sse)));
        assert_stream_hello(&groq_stream).await;

        let ds_ok = deepseek::DeepSeekProvider::from_base(Some(&serve_once(
            "200 OK",
            &json,
            "application/json",
        )));
        assert_complete_ok(&ds_ok).await;
        let ds_err = deepseek::DeepSeekProvider::from_base(Some(&serve_once(
            "500 Internal Server Error",
            "{}",
            "application/json",
        )));
        assert_complete_err(&ds_err).await;
        let ds_stream = deepseek::DeepSeekProvider::from_base(Some(&serve_sse(&sse)));
        assert_stream_hello(&ds_stream).await;

        let mi_ok = mistral::MistralProvider::from_base(Some(&serve_once(
            "200 OK",
            &json,
            "application/json",
        )));
        assert_complete_ok(&mi_ok).await;
        let mi_err = mistral::MistralProvider::from_base(Some(&serve_once(
            "403 Forbidden",
            "{}",
            "application/json",
        )));
        assert_complete_err(&mi_err).await;
        let mi_stream = mistral::MistralProvider::from_base(Some(&serve_sse(&sse)));
        assert_stream_hello(&mi_stream).await;

        let to_ok = together::TogetherProvider::from_base(Some(&serve_once(
            "200 OK",
            &json,
            "application/json",
        )));
        assert_complete_ok(&to_ok).await;
        let to_err = together::TogetherProvider::from_base(Some(&serve_once(
            "429 Too Many Requests",
            "{}",
            "application/json",
        )));
        assert_complete_err(&to_err).await;
        let to_stream = together::TogetherProvider::from_base(Some(&serve_sse(&sse)));
        assert_stream_hello(&to_stream).await;

        let or_ok = openrouter::OpenRouterProvider::from_base(Some(&serve_once(
            "200 OK",
            &json,
            "application/json",
        )))
        .with_site("https://example.test".into(), "WhyCodes".into());
        assert_complete_ok(&or_ok).await;
        let or_err = openrouter::OpenRouterProvider::from_base(Some(&serve_once(
            "401 Unauthorized",
            "{}",
            "application/json",
        )));
        assert_complete_err(&or_err).await;
        let or_stream = openrouter::OpenRouterProvider::from_base(Some(&serve_sse(&sse)));
        assert_stream_hello(&or_stream).await;

        assert!(
            groq::GroqProvider::default()
                .default_base_url()
                .contains("groq.com")
        );
        assert!(
            deepseek::DeepSeekProvider::default()
                .default_base_url()
                .contains("deepseek.com")
        );
        assert!(
            mistral::MistralProvider::default()
                .default_base_url()
                .contains("mistral")
        );
        assert!(
            together::TogetherProvider::default()
                .default_base_url()
                .contains("together")
        );
        assert!(
            openrouter::OpenRouterProvider::default()
                .default_base_url()
                .contains("openrouter")
        );
    }
}
