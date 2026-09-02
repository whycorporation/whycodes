/// OpenAI-compatible LLM provider.
/// Works with OpenAI, xAI (Grok), DeepSeek, OpenRouter, and other
/// OpenAI-compatible APIs.
use async_stream::stream;
use serde_json::Value;
use whycodes_core::types::{LlmRequest, LlmResponse, StreamEvent};

use crate::provider::{
    LlmProvider, ProviderEventStream, ProviderResponseFuture, ProviderStreamFuture,
};

pub struct OpenAiProvider {
    name: String,
    chat_url: String,
}

impl OpenAiProvider {
    pub fn new() -> Self {
        Self::from_base(None)
    }

    pub fn from_config(config: &whycodes_core::types::ProviderConfig) -> Self {
        Self::from_base(config.base_url.as_deref().or(config.api_base.as_deref()))
    }

    pub fn from_base(base: Option<&str>) -> Self {
        Self {
            name: "openai".to_string(),
            chat_url: match base.map(str::trim).filter(|s| !s.is_empty()) {
                Some(raw) => super::custom::normalize_chat_completions_url(raw),
                None => "https://api.openai.com/v1/chat/completions".to_string(),
            },
        }
    }

    pub fn build_body(&self, request: &LlmRequest, model: &str) -> Value {
        let mut body = serde_json::json!({
            "model": model,
            "messages": self.convert_messages(request),
            "stream": true,
        });

        if let Some(max_tokens) = request.max_tokens {
            body["max_tokens"] = max_tokens.into();
        }

        if !request.tools.is_empty() {
            body["tools"] = serde_json::Value::Array(self.convert_tools(&request.tools));
            body["tool_choice"] = serde_json::json!("auto");
            // Encourage the model to emit independent tool calls in one step
            // so our agent can fan them out (Codex / OpenAI latency guide).
            body["parallel_tool_calls"] = serde_json::json!(true);
        }

        crate::openai_compat::apply_sampling(&mut body, request);

        crate::thinking::ThinkingConfig::apply_openai_effort(&mut body, request.thinking.as_ref());

        body
    }

    fn convert_messages(&self, request: &LlmRequest) -> Vec<Value> {
        crate::openai_compat::convert_messages(request)
    }

    fn convert_tools(&self, tools: &[whycodes_core::types::ToolDefinition]) -> Vec<Value> {
        crate::openai_compat::convert_tools(tools)
    }
}

fn openai_post(url: &str, api_key: &str) -> reqwest::RequestBuilder {
    let req = crate::client_identity::post(url);
    let key = api_key.trim();
    if key.is_empty() {
        req
    } else {
        req.header("Authorization", format!("Bearer {key}"))
    }
}

impl LlmProvider for OpenAiProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_base_url(&self) -> &str {
        &self.chat_url
    }

    fn complete<'a>(
        &'a self,
        request: &'a LlmRequest,
        api_key: &'a str,
        model: &'a str,
    ) -> ProviderResponseFuture<'a> {
        Box::pin(async move {
            // ChatGPT-subscription OAuth tokens are rejected by api.openai.com;
            // route them to the Codex backend (Responses API) instead.
            if super::codex::is_chatgpt_oauth_token(api_key) {
                return super::codex::complete(request, api_key, model).await;
            }
            let mut body = self.build_body(request, model);
            body["stream"] = serde_json::Value::Bool(false);

            let resp = openai_post(self.default_base_url(), api_key)
                .json(&body)
                .send()
                .await
                .map_err(|e| whycodes_core::Error::llm(format!("HTTP error: {e}")))?;

            let status = resp.status();
            let json: Value = resp
                .json()
                .await
                .map_err(|e| whycodes_core::Error::llm(format!("JSON parse error: {e}")))?;

            if !status.is_success() {
                let err_msg = json["error"]["message"].as_str().unwrap_or("Unknown error");
                return Err(whycodes_core::Error::llm(format!(
                    "OpenAI API error ({}): {}",
                    status, err_msg
                )));
            }

            let choice = &json["choices"][0];
            let message = &choice["message"];
            let content = crate::openai_compat::content_blocks_from_chat_message(message);

            let usage = &json["usage"];
            Ok(LlmResponse {
                content,
                stop_reason: choice["finish_reason"].as_str().map(|s| s.to_string()),
                usage: crate::openai_compat::usage_from_chat_completion(usage),
                model: model.to_string(),
            })
        })
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
        api_key: &'a str,
        model: &'a str,
    ) -> ProviderStreamFuture<'a> {
        Box::pin(async move {
            // See `complete`: JWT-shaped subscription tokens go to the Codex
            // backend, API keys keep the chat-completions path.
            if super::codex::is_chatgpt_oauth_token(api_key) {
                return super::codex::stream(request, api_key, model).await;
            }
            let mut body = self.build_body(request, model);
            crate::openai_compat::attach_stream_usage_option(&mut body);

            let resp = openai_post(self.default_base_url(), api_key)
                .json(&body)
                .send()
                .await
                .map_err(|e| whycodes_core::Error::llm(format!("HTTP error: {e}")))?;

            if !resp.status().is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(whycodes_core::Error::llm(format!(
                    "OpenAI API error: {}",
                    text
                )));
            }

            let s = stream! {
                let mut stream = resp.bytes_stream();
                let mut buffer = String::new();

                while let Some(chunk) = futures::StreamExt::next(&mut stream).await {
                    match chunk {
                        Ok(bytes) => {
                            buffer.push_str(&String::from_utf8_lossy(&bytes));
                            while let Some(pos) = buffer.find('\n') {
                                let line = buffer[..pos].trim().to_string();
                                buffer = buffer[pos + 1..].to_string();

                                if line.is_empty() || !line.starts_with("data: ") {
                                    continue;
                                }

                                let data = &line[6..];
                                if data == "[DONE]" {
                                    yield Ok(StreamEvent::MessageStop);
                                    return;
                                }

                                if let Ok(event) = serde_json::from_str::<Value>(data) {
                                    let choice = &event["choices"][0];
                                    let delta = &choice["delta"];

                                    for ev in crate::openai_compat::stream_events_for_chat_delta(delta) {
                                        yield Ok(ev);
                                    }

                                    // Final include_usage chunk often has empty choices —
                                    // do not require finish_reason.
                                    if let Some(ev) =
                                        crate::openai_compat::stream_usage_from_chunk(&event)
                                    {
                                        yield Ok(ev);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            yield Err(crate::openai_compat::stream_chunk_error("openai", e));
                        }
                    }
                }
            };

            Ok(Box::pin(s) as ProviderEventStream)
        })
    }
}

impl Default for OpenAiProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;
    use tokio_stream::StreamExt;
    use whycodes_core::types::{ContentBlock, LlmRequest, Message, MessageContent, Role};

    #[test]
    fn openai_module_loads() {
        assert!(!module_path!().is_empty());
    }

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
        assert!(
            custom.default_base_url().ends_with("/chat/completions"),
            "{}",
            custom.default_base_url()
        );

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
}
