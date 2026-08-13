/// Ollama LLM provider.
/// Uses the Ollama chat API at localhost:11434.
/// Format differs from OpenAI — uses Ollama-specific request/response shapes.
use async_stream::stream;
use futures::stream::Stream;
use serde_json::Value;
use std::pin::Pin;
use whycode_core::types::{ContentBlock, LlmRequest, LlmResponse, StreamEvent, Usage};

use crate::provider::LlmProvider;
use async_trait::async_trait;

pub struct OllamaProvider {
    name: String,
}

impl OllamaProvider {
    pub fn new() -> Self {
        Self {
            name: "ollama".to_string(),
        }
    }

    fn build_body(&self, request: &LlmRequest, model: &str) -> Value {
        let messages = self.convert_messages(request);
        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": true,
        });

        // Ensure options object exists
        if !body.as_object().unwrap().contains_key("options") {
            body["options"] = serde_json::json!({});
        }

        if let Some(temp) = request.temperature {
            body["options"]["temperature"] = temp.into();
        }

        if let Some(max_tokens) = request.max_tokens {
            body["options"]["num_predict"] = max_tokens.into();
        }

        if let Some(top_p) = request.top_p {
            body["options"]["top_p"] = top_p.into();
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
                whycode_core::types::Role::Assistant => "assistant",
                whycode_core::types::Role::User => "user",
                whycode_core::types::Role::System => "system",
                whycode_core::types::Role::Tool => "tool",
            };

            let text = msg.content.as_text().unwrap_or("[content]").to_string();

            let mut msg_obj = serde_json::json!({
                "role": role,
                "content": text,
            });

            // If message has images (from ContentBlock::Image), attach them as Ollama images
            if let whycode_core::types::MessageContent::Blocks(blocks) = &msg.content {
                let mut images: Vec<String> = Vec::new();
                for block in blocks {
                    if let ContentBlock::Image {
                        source: whycode_core::types::ImageSource::Base64 { data, .. },
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

    fn convert_tools(&self, tools: &[whycode_core::types::ToolDefinition]) -> Vec<Value> {
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
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_base_url(&self) -> &str {
        "http://localhost:11434/api/chat"
    }

    async fn complete(
        &self,
        request: &LlmRequest,
        _api_key: &str,
        model: &str,
    ) -> whycode_core::Result<LlmResponse> {
        let mut body = self.build_body(request, model);
        body["stream"] = serde_json::Value::Bool(false);

        let resp = crate::client_identity::post(self.default_base_url())
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
            let err_msg = json["error"].as_str().unwrap_or("Unknown error");
            return Err(whycode_core::Error::Llm(format!(
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
        _api_key: &str,
        model: &str,
    ) -> whycode_core::Result<Pin<Box<dyn Stream<Item = whycode_core::Result<StreamEvent>> + Send>>>
    {
        let body = self.build_body(request, model);

        let resp = crate::client_identity::post(self.default_base_url())
            .json(&body)
            .send()
            .await
            .map_err(|e| whycode_core::Error::Llm(format!("HTTP error: {e}")))?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(whycode_core::Error::Llm(format!(
                "Ollama API error: {}",
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
                        // Ollama streams newline-delimited JSON objects (one per line)
                        while let Some(pos) = buffer.find('\n') {
                            let line = buffer[..pos].trim().to_string();
                            buffer = buffer[pos + 1..].to_string();

                            if line.is_empty() {
                                continue;
                            }

                            if let Ok(event) = serde_json::from_str::<Value>(&line) {
                                // Check for errors
                                if let Some(err) = event.get("error") {
                                    yield Err(whycode_core::Error::Llm(
                                        err.as_str().unwrap_or("Unknown error").to_string(),
                                    ));
                                    return;
                                }

                                let done = event["done"].as_bool().unwrap_or(false);

                                if let Some(message) = event.get("message") {
                                    if let Some(text) = message["content"].as_str()
                                        && !text.is_empty() {
                                            yield Ok(StreamEvent::TextDelta {
                                                text: text.to_string(),
                                            });
                                        }

                                    // Tool calls in streaming may appear in message
                                    if let Some(tool_calls) = message["tool_calls"].as_array() {
                                        for tc in tool_calls {
                                            let func = &tc["function"];
                                            let raw_args = &func["arguments"];
                                            // Ollama may send object or JSON string.
                                            let input = if raw_args.is_string() || raw_args.is_null()
                                            {
                                                crate::openai_compat::parse_tool_arguments(raw_args)
                                            } else {
                                                raw_args.clone()
                                            };
                                            yield Ok(StreamEvent::ToolUse {
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

                                if done {
                                    if let (Some(input), Some(output)) = (
                                        event["prompt_eval_count"].as_u64(),
                                        event["eval_count"].as_u64(),
                                    ) {
                                        yield Ok(StreamEvent::Usage {
                                            input_tokens: input,
                                            output_tokens: output,
                                        });
                                    }
                                    yield Ok(StreamEvent::MessageStop);
                                    return;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        yield Err(whycode_core::Error::Llm(format!("Stream error: {e}")));
                    }
                }
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
