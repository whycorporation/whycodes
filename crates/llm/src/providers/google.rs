/// Google Gemini LLM provider.
/// Uses the Gemini generateContent API with streaming support.
use serde_json::Value;
use whycodes_core::types::{ContentBlock, LlmRequest, LlmResponse, Usage};

use crate::json_value::{self, arr, obj, str as jstr};
use crate::provider::{LlmProvider, ProviderResponseFuture, ProviderStreamFuture};

pub struct GoogleProvider {
    name: String,
    /// Optional override for tests (`from_base`). Production stays on
    /// `generativelanguage.googleapis.com`.
    base_url: Option<String>,
}

impl GoogleProvider {
    pub fn new() -> Self {
        Self::from_base(None)
    }

    pub fn from_base(base: Option<&str>) -> Self {
        Self {
            name: "google".to_string(),
            base_url: base
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        }
    }

    /// Streaming endpoint. The key goes in the `x-goog-api-key` header, not
    /// the query string: reqwest puts the URL in its error text, and that
    /// text reaches the TUI and the logs.
    pub(crate) fn build_url(&self, model: &str) -> String {
        format!(
            "{}/{model}:streamGenerateContent?alt=sse",
            self.models_base()
        )
    }

    pub(crate) fn build_complete_url(&self, model: &str) -> String {
        format!("{}/{model}:generateContent", self.models_base())
    }

    fn models_base(&self) -> String {
        match self.base_url.as_deref() {
            Some(base) => format!("{}/v1beta/models", base.trim_end_matches('/')),
            None => "https://generativelanguage.googleapis.com/v1beta/models".to_string(),
        }
    }
}

/// Header carrying a Gemini API key ([`GoogleProvider::build_url`]).
const API_KEY_HEADER: &str = "x-goog-api-key";

impl LlmProvider for GoogleProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_base_url(&self) -> &str {
        "https://generativelanguage.googleapis.com/v1beta/models"
    }

    fn complete<'a>(
        &'a self,
        request: &'a LlmRequest,
        api_key: &'a str,
        model: &'a str,
    ) -> ProviderResponseFuture<'a> {
        Box::pin(async move {
            // Google OAuth subscription tokens are rejected by the API-key
            // generativelanguage route; send them to the Code Assist endpoint.
            if super::codeassist::is_google_oauth_token(api_key) {
                return super::codeassist::complete(request, api_key, model).await;
            }
            let body = self.build_body(request);

            let url = self.build_complete_url(model);
            let resp = crate::client_identity::post(&url)
                .header(API_KEY_HEADER, api_key)
                .json(&body)
                .send()
                .await
                .map_err(http_error)?;

            let status = resp.status();
            let json: Value = resp.json().await.map_err(json_parse_error)?;

            if !status.is_success() {
                let err_msg = json["error"]["message"].as_str().unwrap_or("Unknown error");
                return Err(whycodes_core::Error::llm(format!(
                    "Google API error ({}): {}",
                    status, err_msg
                )));
            }

            let content = text_blocks_from_candidates(&json);

            let usage = &json["usageMetadata"];
            Ok(LlmResponse {
                content,
                stop_reason: finish_reason_from_candidates(&json),
                usage: Usage {
                    input_tokens: usage["promptTokenCount"].as_u64().unwrap_or(0),
                    output_tokens: usage["candidatesTokenCount"].as_u64().unwrap_or(0),
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                },
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
            // See `complete`: OAuth tokens go to Code Assist, API keys keep
            // the generativelanguage route.
            if super::codeassist::is_google_oauth_token(api_key) {
                return super::codeassist::stream(request, api_key, model).await;
            }
            let mut body = self.build_body(request);
            json_value::insert(&mut body, "generationConfig", obj([]));

            let url = self.build_url(model);
            let resp = crate::client_identity::post(&url)
                .header(API_KEY_HEADER, api_key)
                .json(&body)
                .send()
                .await
                .map_err(http_error)?;

            if !resp.status().is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(whycodes_core::Error::llm(format!(
                    "Google API error: {}",
                    text
                )));
            }

            // `alt=sse`: `data: {GenerateContentResponse}` events, the same
            // shape Code Assist streams (text, function calls, usage).
            Ok(super::codeassist::gemini_sse(
                crate::openai_compat::response_bytes(resp),
            ))
        })
    }
}

impl GoogleProvider {
    pub(crate) fn build_body(&self, request: &LlmRequest) -> Value {
        let mut contents: Vec<Value> = Vec::new();

        // System instruction
        let mut body = obj([("contents", arr([]))]);

        if !request.system.is_empty() {
            json_value::insert(
                &mut body,
                "systemInstruction",
                obj([("parts", arr([obj([("text", jstr(&request.system))])]))]),
            );
        }

        for msg in request.messages.iter() {
            let role = match msg.role {
                whycodes_core::types::Role::User => "user",
                whycodes_core::types::Role::Assistant => "model",
                _ => "user",
            };

            let text = msg
                .content
                .as_text()
                .unwrap_or("[non-text content]")
                .to_string();

            contents.push(obj([
                ("role", jstr(role)),
                ("parts", arr([obj([("text", jstr(text))])])),
            ]));
        }

        body["contents"] = Value::Array(contents);

        if let Some(max_tokens) = request.max_tokens {
            json_value::insert(
                &mut body,
                "generationConfig",
                obj([("maxOutputTokens", Value::from(max_tokens))]),
            );
        }

        if let Some(temp) = request.temperature {
            if body.get("generationConfig").is_none() {
                json_value::insert(&mut body, "generationConfig", obj([]));
            }
            crate::openai_compat::set_json_f64(&mut body["generationConfig"], "temperature", temp);
        }

        if !request.tools.is_empty() {
            let decls: Vec<Value> = request
                .tools
                .iter()
                .map(|t| {
                    obj([
                        ("name", jstr(&t.name)),
                        ("description", jstr(&t.description)),
                        (
                            "parameters",
                            crate::openai_compat::sanitize_schema_for_openai(&t.parameters),
                        ),
                    ])
                })
                .collect();
            json_value::insert(
                &mut body,
                "tools",
                arr([obj([("functionDeclarations", arr(decls))])]),
            );
        }

        body
    }
}

fn http_error(err: impl std::fmt::Display) -> whycodes_core::Error {
    whycodes_core::Error::llm(format!("HTTP error: {err}"))
}

fn json_parse_error(err: impl std::fmt::Display) -> whycodes_core::Error {
    whycodes_core::Error::llm(format!("JSON parse error: {err}"))
}

#[cfg(test)]
pub(crate) fn http_error_for_tests(err: &str) -> whycodes_core::Error {
    http_error(err)
}

#[cfg(test)]
pub(crate) fn json_parse_error_for_tests(err: &str) -> whycodes_core::Error {
    json_parse_error(err)
}

impl Default for GoogleProvider {
    fn default() -> Self {
        Self::new()
    }
}

fn candidate_parts(event: &Value) -> impl Iterator<Item = &Value> {
    event["candidates"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|c| c["content"]["parts"].as_array().into_iter().flatten())
}

fn text_blocks_from_candidates(event: &Value) -> Vec<ContentBlock> {
    candidate_parts(event)
        .filter_map(|part| {
            part["text"].as_str().map(|text| ContentBlock::Text {
                text: text.to_string(),
            })
        })
        .collect()
}

fn finish_reason_from_candidates(event: &Value) -> Option<String> {
    event["candidates"][0]["finishReason"]
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
#[path = "google_tests.rs"]
mod tests;
