/// Mistral AI LLM provider.
/// OpenAI-compatible API at api.mistral.ai.
use async_stream::stream;
use serde_json::Value;
use whycodes_core::types::{LlmRequest, LlmResponse, StreamEvent};

use crate::provider::{
    LlmProvider, ProviderEventStream, ProviderResponseFuture, ProviderStreamFuture,
};

pub struct MistralProvider {
    name: String,
    chat_url: String,
}

impl MistralProvider {
    pub fn new() -> Self {
        Self::from_base(None)
    }

    pub fn from_base(base: Option<&str>) -> Self {
        Self {
            name: "mistral".to_string(),
            chat_url: match base.map(str::trim).filter(|s| !s.is_empty()) {
                Some(raw) => super::custom::normalize_chat_completions_url(raw),
                None => "https://api.mistral.ai/v1/chat/completions".to_string(),
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

impl LlmProvider for MistralProvider {
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
            let mut body = self.build_body(request, model);
            body["stream"] = serde_json::Value::Bool(false);

            let resp = crate::client_identity::post(self.default_base_url())
                .header("Authorization", format!("Bearer {}", api_key))
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
                    "Mistral API error ({}): {}",
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
            let mut body = self.build_body(request, model);
            crate::openai_compat::attach_stream_usage_option(&mut body);

            let resp = crate::client_identity::post(self.default_base_url())
                .header("Authorization", format!("Bearer {}", api_key))
                .json(&body)
                .send()
                .await
                .map_err(|e| whycodes_core::Error::llm(format!("HTTP error: {e}")))?;

            if !resp.status().is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(whycodes_core::Error::llm(format!(
                    "Mistral API error: {}",
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

                                    if let Some(ev) =
                                        crate::openai_compat::stream_usage_from_chunk(&event)
                                    {
                                        yield Ok(ev);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            yield Err(crate::openai_compat::stream_chunk_error("mistral", e));
                        }
                    }
                }
            };

            Ok(Box::pin(s) as ProviderEventStream)
        })
    }
}

impl Default for MistralProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::LlmProvider;
    use whycodes_core::types::{LlmRequest, Message, MessageContent, Role};

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
            max_tokens: None,
            temperature: Some(0.5),
            top_p: Some(0.9),
            top_k: None,
            stop_sequences: None,
            thinking: Some(serde_json::json!({"enabled": true, "reasoning_effort": "low"})),
            use_prompt_cache: false,
        }
    }

    #[test]
    fn from_base_blank_keeps_cloud_and_override_normalizes() {
        let cloud = MistralProvider::from_base(Some("   "));
        assert!(cloud.default_base_url().contains("mistral.ai"));
        let local = MistralProvider::from_base(Some("http://127.0.0.1:9/v1"));
        assert!(
            local.default_base_url().ends_with("/chat/completions"),
            "{}",
            local.default_base_url()
        );
        assert_eq!(MistralProvider::default().name(), "mistral");
    }

    #[test]
    fn build_body_without_tools_applies_sampling_and_effort() {
        let body = MistralProvider::new().build_body(&req(), "mistral-small");
        assert!(body.get("tools").is_none());
        assert_eq!(body["reasoning_effort"], "low");
        assert!(body["temperature"].is_number());
        assert!(body["top_p"].is_number());
    }
}
