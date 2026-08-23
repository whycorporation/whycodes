//! Google/Gemini subscription call routing: the Code Assist endpoint.
//!
//! A Google OAuth token (`whycode auth login google`) is rejected by the
//! API-key `generativelanguage` route; Gemini-subscription calls go to the
//! Code Assist endpoint (`cloudcode-pa.googleapis.com`, the service Gemini
//! CLI uses) with the request wrapped in a `{model, project, request}`
//! envelope. `GoogleProvider` delegates here when the credential is an
//! OAuth access token (`ya29.…`); `AIza…` API keys keep the old path.
//!
//! Code Assist needs a Cloud project id. Resolution order:
//! `GOOGLE_CLOUD_PROJECT` env → `loadCodeAssist` (an already-onboarded
//! account returns its managed project) → `onboardUser` on the free tier
//! (long-running operation, polled). The result is cached process-wide.

use async_stream::stream;
use futures::stream::{Stream, StreamExt};
use serde_json::{Value, json};
use std::pin::Pin;
use std::sync::{OnceLock, RwLock};
use whycode_core::types::{
    ContentBlock, LlmRequest, LlmResponse, MessageContent, Role, StreamEvent, Usage,
};

const BASE: &str = "https://cloudcode-pa.googleapis.com/v1internal";

/// Client metadata the Code Assist service expects (Gemini CLI sends the
/// same shape; the values label an unspecified IDE on the current platform).
fn client_metadata() -> Value {
    json!({
        "ideType": "IDE_UNSPECIFIED",
        "platform": "PLATFORM_UNSPECIFIED",
        "pluginType": "GEMINI",
    })
}

/// True when `key` is a Google OAuth access token rather than an API key
/// (`AIza…`). OAuth tokens are rejected by the generativelanguage route.
pub fn is_google_oauth_token(key: &str) -> bool {
    key.starts_with("ya29.")
}

/// Process-wide cache for the resolved Code Assist project id.
fn project_cache() -> &'static RwLock<Option<String>> {
    static CACHE: OnceLock<RwLock<Option<String>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(None))
}

fn cached_project() -> Option<String> {
    project_cache().read().ok()?.clone()
}

fn cache_project(id: &str) {
    if let Ok(mut guard) = project_cache().write() {
        *guard = Some(id.to_string());
    }
}

/// POST `{BASE}{path}` with the OAuth bearer token; a 401 force-renews the
/// stored credential once via `oauth_refresh` (Google tokens last 1h, so a
/// revoked or early-expired token is the common failure).
async fn post(path: &str, api_key: &str, body: &Value) -> whycode_core::Result<reqwest::Response> {
    crate::oauth_refresh::send_with_refresh_retry("google", api_key, |key| {
        crate::client_identity::post(format!("{BASE}{path}").as_str())
            .header("Authorization", format!("Bearer {key}"))
            .json(body)
    })
    .await
}

/// GET an LRO status (`GET {BASE}/{operation_name}`) with the same retry.
async fn get(path: &str, api_key: &str) -> whycode_core::Result<reqwest::Response> {
    crate::oauth_refresh::send_with_refresh_retry("google", api_key, |key| {
        crate::client_identity::http_client()
            .get(format!("{BASE}{path}").as_str())
            .header("Authorization", format!("Bearer {key}"))
    })
    .await
}

/// Resolve the Code Assist project id for this credential, cached
/// process-wide. See the module docs for the resolution order.
async fn project_id(api_key: &str) -> whycode_core::Result<String> {
    if let Some(cached) = cached_project() {
        return Ok(cached);
    }
    let env_project = std::env::var("GOOGLE_CLOUD_PROJECT")
        .ok()
        .filter(|p| !p.is_empty())
        .or(Some("whycodes".to_string()));

    // 1. loadCodeAssist: an already-onboarded account reports its project.
    let mut load_body = json!({ "metadata": client_metadata() });
    if let Some(p) = &env_project {
        load_body["cloudaicompanionProject"] = Value::String(p.clone());
    }
    let resp = post(":loadCodeAssist", api_key, &load_body).await?;
    let status = resp.status();
    let json: Value = resp
        .json()
        .await
        .map_err(|e| whycode_core::Error::Llm(format!("Code Assist loadCodeAssist: {e}")))?;
    if !status.is_success() {
        let msg = json["error"]["message"].as_str().unwrap_or("unknown error");
        return Err(whycode_core::Error::Llm(format!(
            "Code Assist loadCodeAssist ({status}): {msg}"
        )));
    }
    if let Some(id) = json["cloudaicompanionProject"].as_str() {
        cache_project(id);
        return Ok(id.to_string());
    }
    if json["currentTier"].is_object()
        && let Some(p) = &env_project
    {
        // Paid tier without a reported project: the env project is the one.
        cache_project(p);
        return Ok(p.clone());
    }

    // 2. Not onboarded: pick a tier (free tier unless the user brought a
    // project) and run onboardUser, polling the long-running operation.
    let tier = pick_tier(&json, env_project.is_some());
    let mut onboard = json!({ "tierId": tier, "metadata": client_metadata() });
    if let Some(p) = &env_project {
        onboard["cloudaicompanionProject"] = Value::String(p.clone());
    }
    let resp = post(":onboardUser", api_key, &onboard).await?;
    let status = resp.status();
    let op: Value = resp
        .json()
        .await
        .map_err(|e| whycode_core::Error::Llm(format!("Code Assist onboardUser: {e}")))?;
    if !status.is_success() {
        let msg = op["error"]["message"].as_str().unwrap_or("unknown error");
        return Err(whycode_core::Error::Llm(format!(
            "Code Assist onboardUser ({status}): {msg}"
        )));
    }

    let mut operation = op;
    for _ in 0..10 {
        if operation["done"].as_bool().unwrap_or(false) {
            break;
        }
        let Some(name) = operation["name"].as_str().map(str::to_string) else {
            break;
        };
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let resp = get(&format!("/{name}"), api_key).await?;
        operation = resp
            .json()
            .await
            .map_err(|e| whycode_core::Error::Llm(format!("Code Assist operation poll: {e}")))?;
    }

    let project = operation["response"]["cloudaicompanionProject"]["id"]
        .as_str()
        .or(env_project.as_deref())
        .ok_or_else(|| {
            whycode_core::Error::Llm(
                "Code Assist onboarding did not yield a project id; set GOOGLE_CLOUD_PROJECT"
                    .to_string(),
            )
        })?;
    cache_project(project);
    Ok(project.to_string())
}

/// Tier selection for onboardUser: a user-supplied project needs the tier
/// that accepts one; otherwise the free (managed-project) tier.
fn pick_tier(load_response: &Value, has_project: bool) -> String {
    let tiers = load_response["allowedTiers"].as_array();
    let find = |want_user_project: bool| {
        tiers.and_then(|ts| {
            ts.iter()
                .find(|t| {
                    t["userDefinedCloudaicompanionProject"]
                        .as_bool()
                        .unwrap_or(false)
                        == want_user_project
                })
                .and_then(|t| t["id"].as_str())
                .map(str::to_string)
        })
    };
    if has_project {
        find(true).unwrap_or_else(|| "standard-tier".to_string())
    } else {
        find(false).unwrap_or_else(|| "free-tier".to_string())
    }
}

/// The inner generateContent request. Unlike the API-key path in
/// `google.rs`, this maps tool use/result blocks to Gemini's
/// `functionCall`/`functionResponse` parts — subscription users get working
/// tool calls, not "[non-text content]".
fn build_inner_request(request: &LlmRequest) -> Value {
    // Gemini matches function responses by tool *name*; our ToolResult
    // blocks only carry the call id. Recover names from earlier ToolUse
    // blocks in the same history.
    let mut names: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for m in request.messages.iter() {
        if let MessageContent::Blocks(blocks) = &m.content {
            for b in blocks {
                if let ContentBlock::ToolUse { id, name, .. } = b {
                    names.insert(id.as_str(), name.as_str());
                }
            }
        }
    }

    let mut contents: Vec<Value> = Vec::new();
    for m in request.messages.iter() {
        let role = match m.role {
            Role::Assistant => "model",
            _ => "user",
        };
        let mut parts: Vec<Value> = Vec::new();
        match &m.content {
            MessageContent::Text(text) => {
                if !text.trim().is_empty() {
                    parts.push(json!({ "text": text }));
                }
            }
            MessageContent::Blocks(blocks) => {
                for b in blocks {
                    match b {
                        ContentBlock::Text { text } => parts.push(json!({ "text": text })),
                        ContentBlock::ToolUse { name, input, .. } => parts.push(json!({
                            "functionCall": { "name": name, "args": input }
                        })),
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } => {
                            let name = names.get(tool_use_id.as_str()).copied().unwrap_or("tool");
                            parts.push(json!({
                                "functionResponse": {
                                    "name": name,
                                    "response": { "result": content }
                                }
                            }));
                        }
                        ContentBlock::Image { .. } => {}
                        ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. } => {}
                    }
                }
            }
        }
        if !parts.is_empty() {
            contents.push(json!({ "role": role, "parts": parts }));
        }
    }

    let mut inner = json!({ "contents": contents });
    if !request.system.is_empty() {
        inner["systemInstruction"] = json!({ "parts": [{ "text": request.system }] });
    }
    if !request.tools.is_empty() {
        inner["tools"] = json!([{
            "functionDeclarations": request.tools.iter().map(|t| json!({
                "name": t.name,
                "description": t.description,
                "parameters": crate::openai_compat::sanitize_schema_for_openai(&t.parameters)
            })).collect::<Vec<_>>()
        }]);
    }
    let mut gen_config = json!({});
    if let Some(max_tokens) = request.max_tokens {
        gen_config["maxOutputTokens"] = max_tokens.into();
    }
    if let Some(temp) = request.temperature {
        gen_config["temperature"] =
            Value::Number(serde_json::Number::from_f64(temp as f64).unwrap_or_else(|| 0.into()));
    }
    if !gen_config.as_object().is_none_or(|o| o.is_empty()) {
        inner["generationConfig"] = gen_config;
    }
    inner
}

/// Map one Code Assist SSE chunk (a GenerateContentResponse, tolerating a
/// `{"response": …}` wrapper) to whycode stream events. Pure for tests.
/// `call_seq` mints ids for function calls — Gemini does not send any, but
/// our agent round-trips the id through ToolResult.
fn events_for_chunk(data: &str, call_seq: &mut u64) -> Vec<StreamEvent> {
    let Ok(chunk) = serde_json::from_str::<Value>(data) else {
        return Vec::new();
    };
    let event = if chunk["response"].is_object() {
        &chunk["response"]
    } else {
        &chunk
    };
    let mut events = Vec::new();
    if let Some(candidates) = event["candidates"].as_array() {
        for c in candidates {
            if let Some(parts) = c["content"]["parts"].as_array() {
                for part in parts {
                    if let Some(text) = part["text"].as_str() {
                        events.push(StreamEvent::TextDelta {
                            text: text.to_string(),
                        });
                    }
                    if let Some(call) = part.get("functionCall") {
                        *call_seq += 1;
                        events.push(StreamEvent::ToolUse {
                            id: format!("gcall_{call_seq}"),
                            name: call["name"].as_str().unwrap_or_default().to_string(),
                            input: call["args"].clone(),
                        });
                    }
                }
            }
            if let Some(reason) = c["finishReason"].as_str() {
                events.push(StreamEvent::MessageDelta {
                    delta: json!({"finishReason": reason}),
                });
            }
        }
    }
    if let Some(usage) = event.get("usageMetadata") {
        events.push(StreamEvent::Usage {
            input_tokens: usage["promptTokenCount"].as_u64().unwrap_or(0),
            output_tokens: usage["candidatesTokenCount"].as_u64().unwrap_or(0),
        });
        events.push(StreamEvent::MessageStop);
    }
    events
}

pub async fn complete(
    request: &LlmRequest,
    api_key: &str,
    model: &str,
) -> whycode_core::Result<LlmResponse> {
    let project = project_id(api_key).await?;
    let body = json!({
        "model": model,
        "project": project,
        "request": build_inner_request(request),
    });
    let resp = post(":generateContent", api_key, &body).await?;
    let status = resp.status();
    let json: Value = resp
        .json()
        .await
        .map_err(|e| whycode_core::Error::Llm(format!("Code Assist parse: {e}")))?;
    if !status.is_success() {
        let msg = json["error"]["message"].as_str().unwrap_or("Unknown error");
        return Err(whycode_core::Error::Llm(format!(
            "Code Assist error ({status}): {msg}"
        )));
    }
    let json = if json["response"].is_object() {
        json["response"].clone()
    } else {
        json
    };

    let mut content: Vec<ContentBlock> = Vec::new();
    let mut seq = 0u64;
    if let Some(candidates) = json["candidates"].as_array() {
        for c in candidates {
            if let Some(parts) = c["content"]["parts"].as_array() {
                for part in parts {
                    if let Some(text) = part["text"].as_str() {
                        content.push(ContentBlock::Text {
                            text: text.to_string(),
                        });
                    }
                    if let Some(call) = part.get("functionCall") {
                        seq += 1;
                        content.push(ContentBlock::ToolUse {
                            id: format!("gcall_{seq}"),
                            name: call["name"].as_str().unwrap_or_default().to_string(),
                            input: call["args"].clone(),
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
            .map(str::to_string),
        usage: Usage {
            input_tokens: usage["promptTokenCount"].as_u64().unwrap_or(0),
            output_tokens: usage["candidatesTokenCount"].as_u64().unwrap_or(0),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        },
        model: model.to_string(),
    })
}

pub async fn stream(
    request: &LlmRequest,
    api_key: &str,
    model: &str,
) -> whycode_core::Result<Pin<Box<dyn Stream<Item = whycode_core::Result<StreamEvent>> + Send>>> {
    let project = project_id(api_key).await?;
    let body = json!({
        "model": model,
        "project": project,
        "request": build_inner_request(request),
    });
    let resp = post(":streamGenerateContent?alt=sse", api_key, &body).await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let trimmed: String = text.chars().take(500).collect();
        return Err(whycode_core::Error::Llm(format!(
            "Code Assist error ({status}): {trimmed}"
        )));
    }

    let s = stream! {
        let mut byte_stream = resp.bytes_stream();
        let mut buffer = String::new();
        let mut call_seq = 0u64;
        let mut stopped = false;

        while let Some(chunk) = byte_stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buffer.push_str(&String::from_utf8_lossy(&bytes));
                    while let Some(pos) = buffer.find('\n') {
                        let line = buffer[..pos].trim().to_string();
                        buffer = buffer[pos + 1..].to_string();
                        if line.is_empty() || !line.starts_with("data: ") {
                            continue;
                        }
                        for ev in events_for_chunk(&line[6..], &mut call_seq) {
                            if matches!(ev, StreamEvent::MessageStop) {
                                stopped = true;
                            }
                            yield Ok(ev);
                        }
                    }
                }
                Err(e) => {
                    yield Err(whycode_core::Error::Llm(format!("Stream error: {e}")));
                }
            }
        }
        if !stopped {
            yield Ok(StreamEvent::MessageStop);
        }
    };

    Ok(Box::pin(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use whycode_core::types::{ImageSource, Message, ToolDefinition};

    fn message(role: Role, content: MessageContent) -> Message {
        Message {
            role,
            content,
            tool_call_id: None,
            name: None,
            created_at: None,
        }
    }

    fn empty_request(messages: Vec<Message>) -> LlmRequest {
        LlmRequest {
            system: String::new(),
            messages: std::sync::Arc::from(messages),
            tools: Vec::new(),
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            thinking: None,
            use_prompt_cache: false,
        }
    }

    #[test]
    fn client_metadata_matches_code_assist_contract() {
        assert_eq!(
            client_metadata(),
            json!({
                "ideType": "IDE_UNSPECIFIED",
                "platform": "PLATFORM_UNSPECIFIED",
                "pluginType": "GEMINI",
            })
        );
    }

    #[test]
    fn token_shape_detection() {
        assert!(is_google_oauth_token("ya29.a0AfH6SMBx…"));
        assert!(!is_google_oauth_token("AIzaSyAbc123"));
        assert!(!is_google_oauth_token(""));
    }

    fn history_with_tool_call() -> LlmRequest {
        LlmRequest {
            system: "sys".to_string(),
            messages: std::sync::Arc::from(vec![
                Message {
                    role: Role::User,
                    content: MessageContent::Text("run ls".to_string()),
                    tool_call_id: None,
                    name: None,
                    created_at: None,
                },
                Message {
                    role: Role::Assistant,
                    content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                        id: "gcall_1".to_string(),
                        name: "bash".to_string(),
                        input: json!({"cmd": "ls"}),
                    }]),
                    tool_call_id: None,
                    name: None,
                    created_at: None,
                },
                Message {
                    role: Role::User,
                    content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                        tool_use_id: "gcall_1".to_string(),
                        content: "a.rs".to_string(),
                        is_error: None,
                    }]),
                    tool_call_id: None,
                    name: None,
                    created_at: None,
                },
            ]),
            tools: vec![ToolDefinition {
                name: "bash".to_string(),
                description: "Run a command".to_string(),
                parameters: json!({"type": "object", "properties": {}}),
            }],
            max_tokens: Some(1024),
            temperature: Some(0.5),
            top_p: None,
            top_k: None,
            stop_sequences: None,
            thinking: None,
            use_prompt_cache: true,
        }
    }

    #[test]
    fn inner_request_maps_function_parts() {
        let inner = build_inner_request(&history_with_tool_call());
        assert_eq!(inner["systemInstruction"]["parts"][0]["text"], "sys");
        assert_eq!(inner["generationConfig"]["maxOutputTokens"], 1024);
        assert_eq!(inner["tools"][0]["functionDeclarations"][0]["name"], "bash");

        let contents = inner["contents"].as_array().unwrap();
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[1]["parts"][0]["functionCall"]["name"], "bash");
        assert_eq!(contents[1]["parts"][0]["functionCall"]["args"]["cmd"], "ls");
        // ToolResult must be matched back to its tool *name* via the id map.
        assert_eq!(contents[2]["parts"][0]["functionResponse"]["name"], "bash");
        assert_eq!(
            contents[2]["parts"][0]["functionResponse"]["response"]["result"],
            "a.rs"
        );
    }

    #[test]
    fn inner_request_omits_empty_optional_sections_and_unsupported_blocks() {
        let request = empty_request(vec![
            message(Role::System, MessageContent::Text("   ".to_string())),
            message(
                Role::Assistant,
                MessageContent::Blocks(vec![
                    ContentBlock::Image {
                        source: ImageSource::Base64 {
                            media_type: "image/png".to_string(),
                            data: "AAAA".to_string(),
                        },
                    },
                    ContentBlock::Thinking {
                        text: "private".to_string(),
                        signature: None,
                    },
                    ContentBlock::RedactedThinking {
                        data: "opaque".to_string(),
                    },
                ]),
            ),
        ]);

        assert_eq!(build_inner_request(&request), json!({"contents": []}));
    }

    #[test]
    fn inner_request_uses_fallback_name_for_unmatched_tool_result() {
        let request = empty_request(vec![message(
            Role::Tool,
            MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "missing-call".to_string(),
                content: "failed".to_string(),
                is_error: Some(true),
            }]),
        )]);

        let inner = build_inner_request(&request);
        assert_eq!(inner["contents"][0]["role"], "user");
        assert_eq!(
            inner["contents"][0]["parts"][0]["functionResponse"],
            json!({"name": "tool", "response": {"result": "failed"}})
        );
        assert!(inner.get("systemInstruction").is_none());
        assert!(inner.get("tools").is_none());
        assert!(inner.get("generationConfig").is_none());
    }

    #[test]
    fn chunk_maps_text_function_call_and_usage() {
        let mut seq = 0u64;
        let events = events_for_chunk(
            r#"{"candidates":[{"content":{"parts":[{"text":"hi "},{"functionCall":{"name":"read","args":{"path":"x"}}}]}}],"usageMetadata":{"promptTokenCount":7,"candidatesTokenCount":3}}"#,
            &mut seq,
        );
        assert!(matches!(&events[0], StreamEvent::TextDelta { text } if text == "hi "));
        match &events[1] {
            StreamEvent::ToolUse { id, name, input } => {
                assert_eq!(id, "gcall_1");
                assert_eq!(name, "read");
                assert_eq!(input["path"], "x");
            }
            other => panic!("unexpected: {other:?}"),
        }
        assert!(matches!(
            events[2],
            StreamEvent::Usage {
                input_tokens: 7,
                output_tokens: 3
            }
        ));
        assert!(matches!(events[3], StreamEvent::MessageStop));
    }

    #[test]
    fn chunk_tolerates_response_wrapper() {
        let mut seq = 0u64;
        let events = events_for_chunk(
            r#"{"response":{"candidates":[{"content":{"parts":[{"text":"wrapped"}]}}]}}"#,
            &mut seq,
        );
        assert!(matches!(&events[0], StreamEvent::TextDelta { text } if text == "wrapped"));
    }

    #[test]
    fn malformed_and_structurally_empty_chunks_are_ignored() {
        let mut seq = 41u64;
        assert!(events_for_chunk("not json", &mut seq).is_empty());
        assert!(events_for_chunk(r#"{"candidates":null}"#, &mut seq).is_empty());
        assert_eq!(seq, 41);
    }

    #[test]
    fn chunk_defaults_missing_call_fields_and_usage_counts() {
        let mut seq = 7u64;
        let events = events_for_chunk(
            r#"{"candidates":[{"content":{"parts":[{"functionCall":{}}]},"finishReason":"MAX_TOKENS"}],"usageMetadata":{}}"#,
            &mut seq,
        );

        match &events[0] {
            StreamEvent::ToolUse { id, name, input } => {
                assert_eq!(id, "gcall_8");
                assert!(name.is_empty());
                assert!(input.is_null());
            }
            other => panic!("unexpected: {other:?}"),
        }
        assert!(matches!(
            &events[1],
            StreamEvent::MessageDelta { delta }
                if delta == &json!({"finishReason": "MAX_TOKENS"})
        ));
        assert!(matches!(
            events[2],
            StreamEvent::Usage {
                input_tokens: 0,
                output_tokens: 0
            }
        ));
        assert!(matches!(events[3], StreamEvent::MessageStop));
        assert_eq!(seq, 8);
    }

    #[test]
    fn tier_picking() {
        let load = json!({"allowedTiers": [
            {"id": "free-tier", "userDefinedCloudaicompanionProject": false},
            {"id": "standard-tier", "userDefinedCloudaicompanionProject": true}
        ]});
        assert_eq!(pick_tier(&load, false), "free-tier");
        assert_eq!(pick_tier(&load, true), "standard-tier");
        // No tier list: sane defaults.
        let empty = json!({});
        assert_eq!(pick_tier(&empty, false), "free-tier");
        assert_eq!(pick_tier(&empty, true), "standard-tier");

        // Matching entries without string ids are unusable and fall back.
        let malformed = json!({"allowedTiers": [
            {"id": null, "userDefinedCloudaicompanionProject": false},
            {"id": 7, "userDefinedCloudaicompanionProject": true}
        ]});
        assert_eq!(pick_tier(&malformed, false), "free-tier");
        assert_eq!(pick_tier(&malformed, true), "standard-tier");
    }
}
