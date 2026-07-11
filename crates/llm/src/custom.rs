/// A generic OpenAI-compatible provider that can be configured at runtime.
/// Supports custom base URLs, headers, and authentication schemas.
use async_stream::stream;
use async_trait::async_trait;
use futures::stream::Stream;
use serde_json::Value;
use std::collections::HashMap;
use std::pin::Pin;
use whycode_core::types::{ContentBlock, LlmRequest, LlmResponse, StreamEvent, Usage};

use super::provider::LlmProvider;

/// A provider that works with any OpenAI-compatible API endpoint.
///
/// Configure via config.toml:
/// ```toml
/// [providers.my-custom]
/// name = "my-custom"
/// api_key = "sk-xxx"
/// base_url = "https://api.example.com/v1/chat/completions"
/// headers = { "X-Custom-Header" = "value" }
/// ```
pub struct CustomProvider {
    name: String,
    base_url: String,
    api_key: Option<String>,
    headers: HashMap<String, String>,
}

impl CustomProvider {
    /// Create a new custom provider with full configuration
    pub fn new(
        name: impl Into<String>,
        base_url: impl Into<String>,
        api_key: Option<String>,
        headers: HashMap<String, String>,
    ) -> Self {
        Self {
            name: name.into(),
            base_url: base_url.into(),
            api_key,
            headers,
        }
    }

    /// Create from config
    pub fn from_config(config: &whycode_core::types::ProviderConfig) -> Self {
        let url = config
            .base_url
            .clone()
            .or_else(|| config.api_base.clone())
            .unwrap_or_else(|| format!("https://api.{}/v1/chat/completions", config.name));

        let mut headers = config.headers.clone().unwrap_or_default();
        // Add auth header if not already present
        if !headers.contains_key("Authorization")
            && let Some(key) = &config.api_key {
                headers.insert("Authorization".to_string(), format!("Bearer {}", key));
            }

        Self {
            name: config.name.clone(),
            base_url: url,
            api_key: config.api_key.clone(),
            headers,
        }
    }

    fn build_body(&self, request: &LlmRequest, model: &str) -> Value {
        let mut body = serde_json::json!({
            "model": model,
            "messages": self.convert_messages(request),
            "stream": true,
        });

        if request.max_tokens.is_some() {
            body["max_tokens"] = request.max_tokens.unwrap().into();
        }

        if !request.tools.is_empty() {
            body["tools"] = serde_json::Value::Array(self.convert_tools(&request.tools));
            body["tool_choice"] = serde_json::json!("auto");
        }

        if let Some(temp) = request.temperature {
            body["temperature"] =
                Value::Number(serde_json::Number::from_f64(temp as f64).unwrap());
        }
        if let Some(top_p) = request.top_p {
            body["top_p"] = Value::Number(serde_json::Number::from_f64(top_p as f64).unwrap());
        }

        body
    }

    fn convert_messages(&self, request: &LlmRequest) -> Vec<Value> {
        let mut messages: Vec<Value> = Vec::new();
        if !request.system.is_empty() {
            messages.push(serde_json::json!({"role": "system", "content": request.system}));
        }
        for msg in &request.messages {
            let role = match msg.role {
                whycode_core::types::Role::Assistant => "assistant",
                whycode_core::types::Role::User => "user",
                whycode_core::types::Role::System => "system",
                whycode_core::types::Role::Tool => "tool",
            };
            let content = match &msg.content {
                whycode_core::types::MessageContent::Text(text) => Value::String(text.clone()),
                whycode_core::types::MessageContent::Blocks(blocks) => Value::Array(
                    blocks
                        .iter()
                        .map(|b| match b {
                            ContentBlock::Text { text } => {
                                serde_json::json!({"type": "text", "text": text})
                            }
                            ContentBlock::ToolResult {
                                tool_use_id, content, ..
                            } => {
                                serde_json::json!({"tool_call_id": tool_use_id, "content": content})
                            }
                            _ => serde_json::json!({"type": "text", "text": "[block]"}),
                        })
                        .collect(),
                ),
            };
            let mut obj = serde_json::json!({"role": role, "content": content});
            if let Some(tcid) = &msg.tool_call_id {
                obj["tool_call_id"] = Value::String(tcid.clone());
            }
            messages.push(obj);
        }
        messages
    }

    fn convert_tools(&self, tools: &[whycode_core::types::ToolDefinition]) -> Vec<Value> {
        tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters
                    }
                })
            })
            .collect()
    }

    fn build_request(&self, body: &Value) -> reqwest::RequestBuilder {
        let client = reqwest::Client::new();
        let mut req = client.post(&self.base_url).json(body);

        for (key, value) in &self.headers {
            req = req.header(key, value);
        }

        // Fallback auth if no Authorization header set
        if !self.headers.contains_key("Authorization")
            && let Some(key) = &self.api_key {
                req = req.header("Authorization", format!("Bearer {}", key));
            }

        req
    }
}

#[async_trait]
impl LlmProvider for CustomProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_base_url(&self) -> &str {
        &self.base_url
    }

    async fn complete(
        &self,
        request: &LlmRequest,
        _api_key: &str,
        model: &str,
    ) -> whycode_core::Result<LlmResponse> {
        let mut body = self.build_body(request, model);
        body["stream"] = Value::Bool(false);

        let resp = self
            .build_request(&body)
            .send()
            .await
            .map_err(|e| whycode_core::Error::Llm(format!("HTTP error: {e}")))?;

        let status = resp.status();
        let json: Value = resp
            .json()
            .await
            .map_err(|e| whycode_core::Error::Llm(format!("JSON: {e}")))?;

        if !status.is_success() {
            let msg = json["error"]["message"].as_str().unwrap_or("unknown");
            return Err(whycode_core::Error::Llm(format!(
                "{} API error ({}): {}",
                self.name, status, msg
            )));
        }

        let choice = &json["choices"][0];
        let msg = &choice["message"];
        let mut content: Vec<ContentBlock> = Vec::new();
        if let Some(text) = msg["content"].as_str()
            && !text.is_empty() {
                content.push(ContentBlock::Text { text: text.to_string() });
            }
        if let Some(tcs) = msg["tool_calls"].as_array() {
            for tc in tcs {
                let f = &tc["function"];
                content.push(ContentBlock::ToolUse {
                    id: tc["id"].as_str().unwrap_or("").to_string(),
                    name: f["name"].as_str().unwrap_or("").to_string(),
                    input: f["arguments"].clone(),
                });
            }
        }

        let usage = &json["usage"];
        Ok(LlmResponse {
            content,
            stop_reason: choice["finish_reason"].as_str().map(|s| s.to_string()),
            usage: Usage {
                input_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0),
                output_tokens: usage["completion_tokens"].as_u64().unwrap_or(0),
                cache_read_input_tokens: usage["prompt_tokens_details"]["cached_tokens"].as_u64(),
                cache_creation_input_tokens: None,
            },
            model: model.to_string(),
        })
    }

    async fn stream(
        &self,
        request: &LlmRequest,
        _api_key: &str,
        model: &str,
    ) -> whycode_core::Result<Pin<Box<dyn Stream<Item = whycode_core::Result<StreamEvent>> + Send>>>
    {
        let body = self.build_body(request, model);
        let resp = self
            .build_request(&body)
            .send()
            .await
            .map_err(|e| whycode_core::Error::Llm(format!("HTTP: {e}")))?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(whycode_core::Error::Llm(text));
        }

        let s = stream! {
            let mut stream = resp.bytes_stream();
            let mut buf = String::new();
            while let Some(chunk) = futures::StreamExt::next(&mut stream).await {
                match chunk {
                    Ok(bytes) => {
                        buf.push_str(&String::from_utf8_lossy(&bytes));
                        while let Some(pos) = buf.find('\n') {
                            let line = buf[..pos].trim().to_string();
                            buf = buf[pos + 1..].to_string();
                            if line.is_empty() || !line.starts_with("data: ") { continue; }
                            let data = &line[6..];
                            if data == "[DONE]" { yield Ok(StreamEvent::MessageStop); return; }
                            if let Ok(evt) = serde_json::from_str::<Value>(data) {
                                let delta = &evt["choices"][0]["delta"];
                                if let Some(t) = delta["content"].as_str()
                                    && !t.is_empty() { yield Ok(StreamEvent::TextDelta { text: t.to_string() }); }
                                if let Some(tcs) = delta["tool_calls"].as_array() {
                                    let tc = &tcs[0];
                                    if let Some(id) = tc["id"].as_str() {
                                        yield Ok(StreamEvent::ToolUse { id: id.to_string(), name: tc["function"]["name"].as_str().unwrap_or("").to_string(), input: tc["function"]["arguments"].clone() });
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => { yield Err(whycode_core::Error::Llm(format!("Stream: {e}"))); }
                }
            }
        };
        Ok(Box::pin(s))
    }
}

/// Test for custom provider with auth modes
#[cfg(test)]
mod tests {
    use super::*;

    fn make_req() -> LlmRequest {
        LlmRequest {
            system: "You are helpful.".to_string(),
            messages: vec![],
            tools: vec![],
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            thinking: None,
        }
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
        assert_eq!(p.default_base_url(), "https://api.example.com/v1/chat/completions");
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
        req.tools = vec![whycode_core::types::ToolDefinition {
            name: "read".to_string(),
            description: "read file".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }];
        let body = p.build_body(&req, "m");
        assert!(body["tools"].as_array().unwrap().len() == 1);
        assert_eq!(body["tool_choice"], "auto");
    }
}
