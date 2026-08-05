/// A generic OpenAI-compatible provider that can be configured at runtime.
/// Supports custom base URLs, headers, and authentication schemas.
use async_stream::stream;
use async_trait::async_trait;
use futures::stream::Stream;
use serde_json::Value;
use std::collections::HashMap;
use std::pin::Pin;
use whycode_core::types::{
    ContentBlock, LlmRequest, LlmResponse, StreamEvent, ToolArgumentsFormat, Usage,
};

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
/// # Only if the gateway requires bare JSON objects for tool args:
/// # tool_arguments = "object"
/// ```
pub struct CustomProvider {
    name: String,
    base_url: String,
    api_key: Option<String>,
    headers: HashMap<String, String>,
    tool_arguments: ToolArgumentsFormat,
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
            tool_arguments: ToolArgumentsFormat::JsonString,
        }
    }

    /// Create from config
    pub fn from_config(config: &whycode_core::types::ProviderConfig) -> Self {
        // Accept either a bare `/v1` base or a full chat-completions URL.
        let url = normalize_chat_completions_url(
            config
                .base_url
                .as_deref()
                .or(config.api_base.as_deref())
                .unwrap_or("https://api.openai.com/v1"),
        );

        let mut headers = config.headers.clone().unwrap_or_default();
        // Add auth header if not already present
        if !headers.contains_key("Authorization")
            && let Some(key) = &config.api_key
            && !key.is_empty()
        {
            headers.insert("Authorization".to_string(), format!("Bearer {key}"));
        }

        Self {
            name: config.name.clone(),
            base_url: url,
            api_key: config.api_key.clone(),
            headers,
            tool_arguments: config.tool_arguments_format(),
        }
    }

    fn build_body(&self, request: &LlmRequest, model: &str) -> Value {
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

        if let Some(temp) = request.temperature {
            body["temperature"] = Value::Number(serde_json::Number::from_f64(temp as f64).unwrap());
        }
        if let Some(top_p) = request.top_p {
            body["top_p"] = Value::Number(serde_json::Number::from_f64(top_p as f64).unwrap());
        }

        body
    }

    fn convert_messages(&self, request: &LlmRequest) -> Vec<Value> {
        super::openai_compat::convert_messages_with_format(request, self.tool_arguments)
    }

    fn convert_tools(&self, tools: &[whycode_core::types::ToolDefinition]) -> Vec<Value> {
        super::openai_compat::convert_tools(tools)
    }

    fn build_request(&self, body: &Value) -> reqwest::RequestBuilder {
        // Identity first; config `headers` may override (e.g. custom User-Agent).
        let mut req = super::client_identity::post(&self.base_url).json(body);

        for (key, value) in &self.headers {
            req = req.header(key, value);
        }

        // Fallback auth if no Authorization header set
        if !self.headers.contains_key("Authorization")
            && let Some(key) = &self.api_key
            && !key.is_empty()
        {
            req = req.header("Authorization", format!("Bearer {key}"));
        }

        req
    }
}

/// Ensure `base` points at the OpenAI chat-completions endpoint.
///
/// Configs usually store `http://host:port/v1`; the HTTP client posts to
/// `{base}/chat/completions`. If the path already ends with that suffix, leave
/// it alone.
pub fn normalize_chat_completions_url(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    if base.ends_with("/chat/completions") {
        base.to_string()
    } else {
        format!("{base}/chat/completions")
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
            && !text.is_empty()
        {
            content.push(ContentBlock::Text {
                text: text.to_string(),
            });
        }
        if let Some(tcs) = msg["tool_calls"].as_array() {
            for tc in tcs {
                let f = &tc["function"];
                content.push(ContentBlock::ToolUse {
                    id: tc["id"].as_str().unwrap_or("").to_string(),
                    name: f["name"].as_str().unwrap_or("").to_string(),
                    input: super::openai_compat::parse_tool_arguments(&f["arguments"]),
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
                                    for ev in super::openai_compat::stream_events_for_tool_calls(tcs) {
                                        yield Ok(ev);
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
        let pc = whycode_core::types::ProviderConfig {
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
        let pc = whycode_core::types::ProviderConfig {
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
        use whycode_core::types::{ContentBlock, Message, MessageContent, Role};
        req.messages = vec![Message {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: "c1".into(),
                name: "websearch".into(),
                input: serde_json::json!({"query": "nuxt"}),
            }]),
            tool_call_id: None,
            name: None,
        }];
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
