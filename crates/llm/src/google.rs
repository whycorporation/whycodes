/// Google Gemini LLM provider.
/// Uses the Gemini generateContent API with streaming support.
use async_stream::stream;
use futures::stream::Stream;
use serde_json::Value;
use std::pin::Pin;
use whycode_core::types::{ContentBlock, LlmRequest, LlmResponse, StreamEvent, Usage};

use super::provider::LlmProvider;
use async_trait::async_trait;

pub struct GoogleProvider {
    name: String,
}

impl GoogleProvider {
    pub fn new() -> Self {
        Self {
            name: "google".to_string(),
        }
    }

    fn build_url(&self, model: &str, api_key: &str) -> String {
        format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:{}?key={}",
            model, "streamGenerateContent?alt=sse", api_key
        )
    }

    fn build_complete_url(&self, model: &str, api_key: &str) -> String {
        format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            model, api_key
        )
    }
}

#[async_trait]
impl LlmProvider for GoogleProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_base_url(&self) -> &str {
        "https://generativelanguage.googleapis.com/v1beta/models"
    }

    async fn complete(
        &self,
        request: &LlmRequest,
        api_key: &str,
        model: &str,
    ) -> whycode_core::Result<LlmResponse> {
        let client = reqwest::Client::new();
        let body = self.build_body(request);

        let url = self.build_complete_url(model, api_key);
        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| whycode_core::Error::Llm(format!("HTTP error: {e}")))?;

        let status = resp.status();
        let json: Value = resp
            .json()
            .await
            .map_err(|e| whycode_core::Error::Llm(format!("JSON parse error: {e}")))?;

        if !status.is_success() {
            let err_msg = json["error"]["message"].as_str().unwrap_or("Unknown error");
            return Err(whycode_core::Error::Llm(format!(
                "Google API error ({}): {}",
                status, err_msg
            )));
        }

        let mut content: Vec<ContentBlock> = Vec::new();
        if let Some(candidates) = json["candidates"].as_array() {
            for c in candidates {
                if let Some(parts) = c["content"]["parts"].as_array() {
                    for part in parts {
                        if let Some(text) = part["text"].as_str() {
                            content.push(ContentBlock::Text {
                                text: text.to_string(),
                            });
                        }
                    }
                }
            }
        }

        let usage = &json["usageMetadata"];
        Ok(LlmResponse {
            content,
            stop_reason: json["candidates"][0]["finishReason"]
                .as_str()
                .map(|s| s.to_string()),
            usage: Usage {
                input_tokens: usage["promptTokenCount"].as_u64().unwrap_or(0),
                output_tokens: usage["candidatesTokenCount"].as_u64().unwrap_or(0),
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            },
            model: model.to_string(),
        })
    }

    async fn stream(
        &self,
        request: &LlmRequest,
        api_key: &str,
        model: &str,
    ) -> whycode_core::Result<Pin<Box<dyn Stream<Item = whycode_core::Result<StreamEvent>> + Send>>>
    {
        let client = reqwest::Client::new();
        let mut body = self.build_body(request);
        body["generationConfig"] = serde_json::json!({});

        let url = self.build_url(model, api_key);
        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| whycode_core::Error::Llm(format!("HTTP error: {e}")))?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(whycode_core::Error::Llm(format!(
                "Google API error: {}",
                text
            )));
        }

        let s = stream! {
            // Gemini SSE format is different: starts with '[{' per chunk, not 'data: '
            let text = resp.text().await.unwrap_or_default();
            // Remove leading '[' and split by '},{' pattern
            let clean = text.replace("}\n,{", "}\n~{").replace("]\n", "");
            for item_str in clean.split("}\n~").filter(|s| !s.is_empty()) {
                let item = if item_str.starts_with('{') {
                    item_str.to_string()
                } else {
                    format!("{{{}}}", item_str.replace("]}", "}").replace("]\n", ""))
                };

                if let Ok(event) = serde_json::from_str::<Value>(&item) {
                    if let Some(candidates) = event["candidates"].as_array() {
                        for c in candidates {
                            if let Some(parts) = c["content"]["parts"].as_array() {
                                for part in parts {
                                    if let Some(text) = part["text"].as_str() {
                                        yield Ok(StreamEvent::TextDelta {
                                            text: text.to_string(),
                                        });
                                    }
                                }
                            }
                            if let Some(reason) = c["finishReason"].as_str() {
                                yield Ok(StreamEvent::MessageDelta {
                                    delta: serde_json::json!({"finishReason": reason}),
                                });
                            }
                        }
                    }
                    if let Some(usage) = event.get("usageMetadata") {
                        yield Ok(StreamEvent::Usage {
                            input_tokens: usage["promptTokenCount"].as_u64().unwrap_or(0),
                            output_tokens: usage["candidatesTokenCount"].as_u64().unwrap_or(0),
                        });
                        yield Ok(StreamEvent::MessageStop);
                    }
                }
            }
        };

        Ok(Box::pin(s))
    }
}

impl GoogleProvider {
    fn build_body(&self, request: &LlmRequest) -> Value {
        let mut contents: Vec<Value> = Vec::new();

        // System instruction
        let mut body = serde_json::json!({
            "contents": [],
        });

        if !request.system.is_empty() {
            body["systemInstruction"] = serde_json::json!({
                "parts": [{"text": request.system}]
            });
        }

        for msg in &request.messages {
            let role = match msg.role {
                whycode_core::types::Role::User => "user",
                whycode_core::types::Role::Assistant => "model",
                _ => "user",
            };

            let text = msg
                .content
                .as_text()
                .unwrap_or("[non-text content]")
                .to_string();

            contents.push(serde_json::json!({
                "role": role,
                "parts": [{"text": text}]
            }));
        }

        body["contents"] = Value::Array(contents);

        if let Some(max_tokens) = request.max_tokens {
            body["generationConfig"] = serde_json::json!({
                "maxOutputTokens": max_tokens,
            });
        }

        if let Some(temp) = request.temperature {
            if body.get("generationConfig").is_none() {
                body["generationConfig"] = serde_json::json!({});
            }
            body["generationConfig"]["temperature"] = temp.into();
        }

        if !request.tools.is_empty() {
            body["tools"] = serde_json::json!([{
                "functionDeclarations": request.tools.iter().map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters
                    })
                }).collect::<Vec<_>>()
            }]);
        }

        body
    }
}

impl Default for GoogleProvider {
    fn default() -> Self {
        Self::new()
    }
}
