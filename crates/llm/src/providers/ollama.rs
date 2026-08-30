/// Ollama LLM provider.
///
/// Native chat API (`POST {host}/api/chat`), not OpenAI SSE. Default host is
/// `http://localhost:11434`. Override with config `base_url` / `api_base` or
/// `OLLAMA_HOST` (scheme optional, e.g. `127.0.0.1:4554`).
use async_stream::stream;
use futures::stream::Stream;
use serde_json::Value;
use std::pin::Pin;
use whycodes_core::types::{ContentBlock, LlmRequest, LlmResponse, StreamEvent, Usage};

use crate::provider::LlmProvider;
use async_trait::async_trait;

pub const DEFAULT_OLLAMA_HOST: &str = "http://localhost:11434";

pub struct OllamaProvider {
    name: String,
    chat_url: String,
}

impl OllamaProvider {
    pub fn new() -> Self {
        Self::from_base(None)
    }

    pub fn from_config(config: &whycodes_core::types::ProviderConfig) -> Self {
        Self::from_base(config.base_url.as_deref().or(config.api_base.as_deref()))
    }

    pub fn from_base(base: Option<&str>) -> Self {
        Self {
            name: "ollama".to_string(),
            chat_url: normalize_ollama_chat_url(base),
        }
    }

    pub(crate) fn build_body(&self, request: &LlmRequest, model: &str) -> Value {
        let messages = self.convert_messages(request);
        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": true,
            "options": {},
        });

        if let Some(temp) = request.temperature {
            crate::openai_compat::set_json_f64(&mut body["options"], "temperature", temp);
        }

        if let Some(max_tokens) = request.max_tokens {
            body["options"]["num_predict"] = max_tokens.into();
        }

        if let Some(top_p) = request.top_p {
            crate::openai_compat::set_json_f64(&mut body["options"], "top_p", top_p);
        }

        if !request.tools.is_empty() {
            body["tools"] = serde_json::Value::Array(self.convert_tools(&request.tools));
        }

        body
    }

    fn convert_messages(&self, request: &LlmRequest) -> Vec<Value> {
        let mut messages: Vec<Value> = Vec::new();

        // Ollama expects system as a separate message in the array
        if !request.system.is_empty() {
            messages.push(serde_json::json!({
                "role": "system",
                "content": request.system
            }));
        }

        for msg in request.messages.iter() {
            let role = match msg.role {
                whycodes_core::types::Role::Assistant => "assistant",
                whycodes_core::types::Role::User => "user",
                whycodes_core::types::Role::System => "system",
                whycodes_core::types::Role::Tool => "tool",
            };

            let text = msg.content.as_text().unwrap_or("[content]").to_string();

            let mut msg_obj = serde_json::json!({
                "role": role,
                "content": text,
            });

            // If message has images (from ContentBlock::Image), attach them as Ollama images
            if let whycodes_core::types::MessageContent::Blocks(blocks) = &msg.content {
                let mut images: Vec<String> = Vec::new();
                for block in blocks {
                    if let ContentBlock::Image {
                        source: whycodes_core::types::ImageSource::Base64 { data, .. },
                    } = block
                    {
                        images.push(data.clone());
                    }
                }
                if !images.is_empty() {
                    msg_obj["images"] =
                        Value::Array(images.into_iter().map(Value::String).collect());
                }
            }

            messages.push(msg_obj);
        }

        messages
    }

    fn convert_tools(&self, tools: &[whycodes_core::types::ToolDefinition]) -> Vec<Value> {
        tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": crate::openai_compat::sanitize_schema_for_openai(&t.parameters)
                    }
                })
            })
            .collect()
    }

    fn post_chat(&self, body: &Value, api_key: &str) -> reqwest::RequestBuilder {
        let mut req = crate::client_identity::post(&self.chat_url).json(body);
        let key = api_key.trim();
        if !key.is_empty() {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        req
    }
}

/// Turn a configured host into Ollama's native `POST /api/chat` URL.
///
/// Accepts a bare host (`127.0.0.1:4554`), `http://host:port`, `/api/chat`,
/// or an OpenAI-compat `/v1` URL (stripped back to native chat).
pub fn normalize_ollama_chat_url(base: Option<&str>) -> String {
    let env_host = match std::env::var("OLLAMA_HOST") {
        Ok(v) => Some(v),
        Err(std::env::VarError::NotPresent) => None,
        Err(e) => {
            tracing::debug!(error = %e, "OLLAMA_HOST unreadable");
            None
        }
    };
    normalize_ollama_chat_url_with_env(base, env_host.as_deref())
}

pub(crate) fn normalize_ollama_chat_url_with_env(
    base: Option<&str>,
    env_host: Option<&str>,
) -> String {
    let raw = base
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            env_host
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| DEFAULT_OLLAMA_HOST.to_string());

    let mut url = raw.trim().to_string();
    if !url.contains("://") {
        url = format!("http://{url}");
    }
    let url = url.trim_end_matches('/');

    if url.ends_with("/api/chat") {
        return url.to_string();
    }
    // Configs often store the OpenAI-compat root; native chat is `/api/chat`.
    let host = url
        .trim_end_matches("/v1/chat/completions")
        .trim_end_matches("/chat/completions")
        .trim_end_matches("/v1");
    let host = host.trim_end_matches('/');
    format!("{host}/api/chat")
}

/// Parse one Ollama NDJSON object. `done` is the second return value.
/// Callers check `event["error"]` before invoking this.
fn events_from_ollama_object(event: &Value) -> (Vec<StreamEvent>, bool) {
    let mut out = Vec::new();
    let done = event["done"].as_bool().unwrap_or(false);

    if let Some(message) = event.get("message") {
        if let Some(text) = message["content"].as_str()
            && !text.is_empty()
        {
            out.push(StreamEvent::TextDelta {
                text: text.to_string(),
            });
        }

        if let Some(tool_calls) = message["tool_calls"].as_array() {
            for tc in tool_calls {
                let func = &tc["function"];
                let raw_args = &func["arguments"];
                // Ollama may send object or JSON string.
                let input = if raw_args.is_string() || raw_args.is_null() {
                    crate::openai_compat::parse_tool_arguments(raw_args)
                } else {
                    raw_args.clone()
                };
                out.push(StreamEvent::ToolUse {
                    id: tc
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    name: func["name"].as_str().unwrap_or("").to_string(),
                    input,
                });
            }
        }
    }

    if done
        && let (Some(input), Some(output)) = (
            event["prompt_eval_count"].as_u64(),
            event["eval_count"].as_u64(),
        )
    {
        out.push(StreamEvent::Usage {
            input_tokens: input,
            output_tokens: output,
        });
    }

    (out, done)
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_base_url(&self) -> &str {
        &self.chat_url
    }

    async fn complete(
        &self,
        request: &LlmRequest,
        api_key: &str,
        model: &str,
    ) -> whycodes_core::Result<LlmResponse> {
        let mut body = self.build_body(request, model);
        body["stream"] = serde_json::Value::Bool(false);

        let resp = self
            .post_chat(&body, api_key)
            .send()
            .await
            .map_err(|e| whycodes_core::Error::Llm(format!("HTTP error: {e}")))?;

        let status = resp.status();
        let json: Value = resp
            .json()
            .await
            .map_err(|e| whycodes_core::Error::Llm(format!("JSON parse error: {e}")))?;

        if !status.is_success() {
            let err_msg = json["error"].as_str().unwrap_or("Unknown error");
            return Err(whycodes_core::Error::Llm(format!(
                "Ollama API error ({}): {}",
                status, err_msg
            )));
        }

        let mut content: Vec<ContentBlock> = Vec::new();

        // Ollama response has "message" -> "content"
        let message = &json["message"];
        if let Some(text) = message["content"].as_str()
            && !text.is_empty()
        {
            content.push(ContentBlock::Text {
                text: text.to_string(),
            });
        }

        // Check for tool calls in message
        if let Some(tool_calls) = message["tool_calls"].as_array() {
            for tc in tool_calls {
                let func = &tc["function"];
                content.push(ContentBlock::ToolUse {
                    id: tc
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    name: func["name"].as_str().unwrap_or("").to_string(),
                    input: crate::openai_compat::parse_tool_arguments(&func["arguments"]),
                });
            }
        }

        let done = json["done"].as_bool().unwrap_or(false);
        Ok(LlmResponse {
            content,
            stop_reason: if done { Some("stop".to_string()) } else { None },
            usage: Usage {
                input_tokens: json["prompt_eval_count"].as_u64().unwrap_or(0),
                output_tokens: json["eval_count"].as_u64().unwrap_or(0),
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            },
            model: model.to_string(),
        })
    }

    async fn stream(
        &self,
        request: &LlmRequest,
        api_key: &str,
        model: &str,
    ) -> whycodes_core::Result<Pin<Box<dyn Stream<Item = whycodes_core::Result<StreamEvent>> + Send>>>
    {
        let body = self.build_body(request, model);

        let resp = self
            .post_chat(&body, api_key)
            .send()
            .await
            .map_err(|e| whycodes_core::Error::Llm(format!("HTTP error: {e}")))?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(whycodes_core::Error::Llm(format!(
                "Ollama API error: {}",
                text
            )));
        }

        let s = stream! {
            let mut stream = resp.bytes_stream();
            let mut buffer = String::new();
            let mut stopped = false;

            while let Some(chunk) = futures::StreamExt::next(&mut stream).await {
                match chunk {
                    Ok(bytes) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                        // Ollama streams newline-delimited JSON objects (one per line)
                        while let Some(pos) = buffer.find('\n') {
                            let line = buffer[..pos].trim().to_string();
                            buffer = buffer[pos + 1..].to_string();

                            if line.is_empty() {
                                continue;
                            }

                            match serde_json::from_str::<Value>(&line) {
                                Ok(event) => {
                                    if let Some(err) = event.get("error") {
                                        yield Err(whycodes_core::Error::Llm(
                                            err.as_str().unwrap_or("Unknown error").to_string(),
                                        ));
                                        return;
                                    }
                                    let (events, done) = events_from_ollama_object(&event);
                                    for ev in events {
                                        yield Ok(ev);
                                    }
                                    if done {
                                        yield Ok(StreamEvent::MessageStop);
                                        return;
                                    }
                                }
                                Err(e) => {
                                    tracing::debug!(error = %e, "skipping non-json ollama stream line");
                                    continue;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        yield Err(crate::openai_compat::stream_chunk_error("ollama", e));
                    }
                }
            }

            // Last NDJSON object may omit the trailing newline.
            let leftover = buffer.trim();
            if !leftover.is_empty()
                && let Ok(event) = serde_json::from_str::<Value>(leftover)
            {
                if let Some(err) = event.get("error") {
                    yield Err(whycodes_core::Error::Llm(
                        err.as_str().unwrap_or("Unknown error").to_string(),
                    ));
                    return;
                }
                let (events, done) = events_from_ollama_object(&event);
                for ev in events {
                    yield Ok(ev);
                }
                if done {
                    yield Ok(StreamEvent::MessageStop);
                    stopped = true;
                }
            }

            // Agent loop waits for MessageStop. A closed body without `done`
            // (or a last line without `\n`) used to hang the turn forever.
            if !stopped {
                yield Ok(StreamEvent::MessageStop);
            }
        };

        Ok(Box::pin(s))
    }
}

impl Default for OllamaProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            normalize_ollama_chat_url_with_env(
                Some("http://127.0.0.1:4554/v1/chat/completions"),
                None
            ),
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
}
