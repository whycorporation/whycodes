//! Google/Gemini subscription call routing: the Code Assist endpoint.
//!
//! A Google OAuth token (`whycodes auth login google`) is rejected by the
//! API-key `generativelanguage` route; Gemini-subscription calls go to the
//! Code Assist endpoint (`cloudcode-pa.googleapis.com`, the service Gemini
//! CLI uses) with the request wrapped in a `{model, project, request}`
//! envelope. `GoogleProvider` delegates here when the credential is an
//! OAuth access token (`ya29.…`); `AIza…` API keys keep the old path.
//!
//! Antigravity subscription tokens (`whycodes auth login google-antigravity`)
//! use the same RPC shape against `daily-cloudcode-pa.googleapis.com` with
//! `{ ideType: ANTIGRAVITY }` metadata. The hub User-Agent is not hardcoded;
//! a loaded auth plugin may supply one via `inference.user_agent`.
//!
//! Code Assist needs a Cloud project id. Resolution order:
//! stored OAuth extra `project_id` → `GOOGLE_CLOUD_PROJECT` env →
//! `loadCodeAssist` (an already-onboarded account returns its managed
//! project) → `onboardUser` on the free tier (long-running operation,
//! polled). The result is cached process-wide per OAuth provider.

use crate::json_value::{self, arr, bool as jbool, obj, str as jstr};
use crate::provider::ProviderEventStream;
use futures::stream::Stream;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::sync::{OnceLock, RwLock};
use std::task::{Context, Poll};
use std::time::Duration;
use whycodes_core::types::{
    ContentBlock, LlmRequest, LlmResponse, MessageContent, Role, StreamEvent, Usage,
};

const GEMINI_CLI_BASES: &[&str] = &["https://cloudcode-pa.googleapis.com/v1internal"];
const ANTIGRAVITY_BASES: &[&str] = &[
    "https://daily-cloudcode-pa.googleapis.com/v1internal",
    "https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal",
];

#[derive(Clone, Copy)]
struct Profile {
    oauth_provider: &'static str,
    bases: &'static [&'static str],
    antigravity: bool,
}

const GEMINI_CLI: Profile = Profile {
    oauth_provider: "google",
    bases: GEMINI_CLI_BASES,
    antigravity: false,
};

const ANTIGRAVITY: Profile = Profile {
    oauth_provider: "google-antigravity",
    bases: ANTIGRAVITY_BASES,
    antigravity: true,
};

#[cfg(test)]
thread_local! {
    pub(crate) static TEST_GEMINI_BASES: std::cell::RefCell<Option<&'static [&'static str]>> =
        const { std::cell::RefCell::new(None) };
    pub(crate) static TEST_ANTIGRAVITY_BASES: std::cell::RefCell<Option<&'static [&'static str]>> =
        const { std::cell::RefCell::new(None) };
    pub(crate) static TEST_POLL_SLEEP: std::cell::Cell<Duration> =
        const { std::cell::Cell::new(Duration::ZERO) };
}

fn gemini_cli_profile() -> Profile {
    #[cfg(test)]
    if let Some(bases) = TEST_GEMINI_BASES.with(|c| *c.borrow()) {
        return Profile {
            oauth_provider: "google",
            bases,
            antigravity: false,
        };
    }
    GEMINI_CLI
}

fn antigravity_profile() -> Profile {
    #[cfg(test)]
    if let Some(bases) = TEST_ANTIGRAVITY_BASES.with(|c| *c.borrow()) {
        return Profile {
            oauth_provider: "google-antigravity",
            bases,
            antigravity: true,
        };
    }
    ANTIGRAVITY
}

const PRODUCTION_ONBOARD_POLL: Duration = Duration::from_secs(1);

fn onboard_poll_sleep() -> Duration {
    #[cfg(test)]
    {
        let d = TEST_POLL_SLEEP.with(|c| c.get());
        if d != Duration::MAX {
            return d;
        }
    }
    PRODUCTION_ONBOARD_POLL
}

/// Logical picker ids collapse to a wire id on `daily-cloudcode-pa`. Sending
/// the family name (`gemini-3.1-pro`) 404s; the hub uses the effort member.
fn antigravity_wire_model(model: &str) -> &str {
    match model {
        "gemini-3.1-pro" => "gemini-3.1-pro-low",
        "gemini-3.1-pro-high" => "gemini-pro-agent",
        "gemini-3.1-flash" | "gemini-3-flash" | "gemini-3.5-flash" => "gemini-3.5-flash-low",
        "gemini-3.6-flash" => "gemini-3.6-flash-low",
        "gemini-3.7-flash" => "gemini-3.7-flash-low",
        other => other,
    }
}

/// Client metadata the Code Assist service expects (Gemini CLI sends the
/// same shape; the values label an unspecified IDE on the current platform).
fn client_metadata() -> Value {
    obj([
        ("ideType", jstr("IDE_UNSPECIFIED")),
        ("platform", jstr("PLATFORM_UNSPECIFIED")),
        ("pluginType", jstr("GEMINI")),
    ])
}

fn metadata_for(profile: &Profile) -> Value {
    if profile.antigravity {
        obj([("ideType", jstr("ANTIGRAVITY"))])
    } else {
        client_metadata()
    }
}

/// True when `key` is a Google OAuth access token rather than an API key
/// (`AIza…`). OAuth tokens are rejected by the generativelanguage route.
pub fn is_google_oauth_token(key: &str) -> bool {
    key.starts_with("ya29.")
}

/// Process-wide cache for the resolved Code Assist project id, keyed by
/// OAuth provider (`google` vs `google-antigravity`).
fn project_cache() -> &'static RwLock<HashMap<String, String>> {
    static CACHE: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn cached_project(provider: &str) -> Option<String> {
    project_cache()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(provider)
        .cloned()
}

fn cache_project(provider: &str, id: &str) {
    project_cache()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .insert(provider.to_string(), id.to_string());
}

#[cfg(test)]
pub(crate) fn cache_project_for_tests(provider: &str, id: &str) {
    cache_project(provider, id);
}

fn authorize(
    profile: &Profile,
    req: reqwest::RequestBuilder,
    key: &str,
) -> reqwest::RequestBuilder {
    crate::client_identity::with_plugin_identity(
        req.header("Authorization", format!("Bearer {key}")),
        profile.oauth_provider,
    )
}

fn failover_http(status: u16) -> bool {
    matches!(status, 404 | 408 | 429 | 500 | 502 | 503 | 504)
}

/// POST `{base}{path}` with the OAuth bearer token; a 401 force-renews
/// the stored credential once via `oauth_refresh` (Google tokens last 1h, so a
/// revoked or early-expired token is the common failure).
async fn post_at(
    profile: &Profile,
    base: &str,
    path: &str,
    api_key: &str,
    body: &Value,
) -> whycodes_core::Result<reqwest::Response> {
    let url = format!("{base}{path}");
    crate::oauth_refresh::send_with_refresh_retry(profile.oauth_provider, api_key, |key| {
        authorize(
            profile,
            crate::client_identity::http_client().post(&url),
            key,
        )
        .header("Content-Type", "application/json")
        .json(body)
    })
    .await
}

async fn post(
    profile: &Profile,
    path: &str,
    api_key: &str,
    body: &Value,
) -> whycodes_core::Result<reqwest::Response> {
    post_at(profile, profile.bases[0], path, api_key, body).await
}

/// Like [`post`], but Antigravity generate/stream calls try the sandbox host
/// when daily answers a failover status (404 is the common "this RPC isn't
/// on this frontend" miss).
async fn post_generate(
    profile: &Profile,
    path: &str,
    api_key: &str,
    body: &Value,
) -> whycodes_core::Result<reqwest::Response> {
    let mut iter = profile.bases.iter().enumerate().peekable();
    loop {
        let Some((_, base)) = iter.next() else {
            return Err(whycodes_core::Error::llm(
                "Code Assist generate: no endpoints configured".to_string(),
            ));
        };
        let last = iter.peek().is_none();
        let resp = post_at(profile, base, path, api_key, body).await?;
        if resp.status().is_success() || last || !failover_http(resp.status().as_u16()) {
            return Ok(resp);
        }
        let _ = resp.bytes().await;
    }
}

/// GET an LRO status (`GET {base}/{operation_name}`) with the same retry.
async fn get(
    profile: &Profile,
    path: &str,
    api_key: &str,
) -> whycodes_core::Result<reqwest::Response> {
    let url = format!("{}{path}", profile.bases[0]);
    crate::oauth_refresh::send_with_refresh_retry(profile.oauth_provider, api_key, |key| {
        authorize(
            profile,
            crate::client_identity::http_client().get(&url),
            key,
        )
    })
    .await
}

/// Resolve the Code Assist project id for this credential, cached
/// process-wide per OAuth provider. See the module docs for the resolution
/// order.
async fn project_id(profile: &Profile, api_key: &str) -> whycodes_core::Result<String> {
    if let Some(cached) = cached_project(profile.oauth_provider) {
        return Ok(cached);
    }
    if let Some(id) = crate::oauth_refresh::stored_extra(profile.oauth_provider, "project_id")
        .await
        .filter(|p| !p.is_empty())
    {
        cache_project(profile.oauth_provider, &id);
        return Ok(id);
    }
    let env_project = std::env::var("GOOGLE_CLOUD_PROJECT")
        .ok()
        .filter(|p| !p.is_empty())
        .or_else(|| {
            if profile.antigravity {
                None
            } else {
                Some("whycodes".to_string())
            }
        });

    // 1. loadCodeAssist: an already-onboarded account reports its project.
    let mut load_body = obj([("metadata", metadata_for(profile))]);
    if let Some(p) = &env_project {
        load_body["cloudaicompanionProject"] = Value::String(p.clone());
    }
    let resp = post(profile, ":loadCodeAssist", api_key, &load_body).await?;
    let status = resp.status();
    let json: Value = resp
        .json()
        .await
        .map_err(|e| whycodes_core::Error::llm(format!("Code Assist loadCodeAssist: {e}")))?;
    if !status.is_success() {
        let msg = json["error"]["message"].as_str().unwrap_or("unknown error");
        return Err(whycodes_core::Error::llm(format!(
            "Code Assist loadCodeAssist ({status}): {msg}"
        )));
    }
    if let Some(id) = json["cloudaicompanionProject"].as_str() {
        cache_project(profile.oauth_provider, id);
        return Ok(id.to_string());
    }
    if json["currentTier"].is_object()
        && let Some(p) = &env_project
    {
        // Paid tier without a reported project: the env project is the one.
        cache_project(profile.oauth_provider, p);
        return Ok(p.clone());
    }

    // 2. Not onboarded: pick a tier (free tier unless the user brought a
    // project) and run onboardUser, polling the long-running operation.
    let tier = pick_tier(&json, env_project.is_some());
    let mut onboard = obj([("tierId", jstr(tier)), ("metadata", metadata_for(profile))]);
    if let Some(p) = &env_project {
        onboard["cloudaicompanionProject"] = Value::String(p.clone());
    }
    let resp = post(profile, ":onboardUser", api_key, &onboard).await?;
    let status = resp.status();
    let op: Value = resp
        .json()
        .await
        .map_err(|e| whycodes_core::Error::llm(format!("Code Assist onboardUser: {e}")))?;
    if !status.is_success() {
        let msg = op["error"]["message"].as_str().unwrap_or("unknown error");
        return Err(whycodes_core::Error::llm(format!(
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
        tokio::time::sleep(onboard_poll_sleep()).await;
        let resp = get(profile, &format!("/{name}"), api_key).await?;
        operation = resp
            .json()
            .await
            .map_err(|e| whycodes_core::Error::llm(format!("Code Assist operation poll: {e}")))?;
    }

    let project = operation["response"]["cloudaicompanionProject"]["id"]
        .as_str()
        .or(env_project.as_deref())
        .ok_or_else(|| {
            whycodes_core::Error::llm(
                "Code Assist onboarding did not yield a project id; set GOOGLE_CLOUD_PROJECT"
                    .to_string(),
            )
        })?;
    cache_project(profile.oauth_provider, project);
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
                    parts.push(obj([("text", jstr(text))]));
                }
            }
            MessageContent::Blocks(blocks) => {
                for b in blocks {
                    match b {
                        ContentBlock::Text { text } => parts.push(obj([("text", jstr(text))])),
                        ContentBlock::ToolUse { name, input, .. } => parts.push(obj([(
                            "functionCall",
                            obj([("name", jstr(name)), ("args", input.clone())]),
                        )])),
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } => {
                            let name = names.get(tool_use_id.as_str()).copied().unwrap_or("tool");
                            parts.push(obj([(
                                "functionResponse",
                                obj([
                                    ("name", jstr(name)),
                                    ("response", obj([("result", jstr(content))])),
                                ]),
                            )]));
                        }
                        ContentBlock::Image { .. } => {}
                        ContentBlock::Thinking { text, signature } => {
                            // Claude-via-Antigravity 400s if thinking is
                            // dropped and later replayed as `text`. Echo the
                            // Gemini thought part (and signature when we have
                            // one) so the converted Anthropic history stays valid.
                            let mut part = obj([("text", jstr(text)), ("thought", jbool(true))]);
                            if let Some(sig) = signature
                                && !sig.is_empty()
                            {
                                json_value::insert(&mut part, "thoughtSignature", jstr(sig));
                            }
                            parts.push(part);
                        }
                        ContentBlock::RedactedThinking { data } => {
                            if !data.is_empty() {
                                parts.push(obj([("text", jstr(data)), ("thought", jbool(true))]));
                            }
                        }
                    }
                }
            }
        }
        if !parts.is_empty() {
            contents.push(obj([("role", jstr(role)), ("parts", arr(parts))]));
        }
    }

    let mut inner = obj([("contents", arr(contents))]);
    if !request.system.is_empty() {
        json_value::insert(
            &mut inner,
            "systemInstruction",
            obj([("parts", arr([obj([("text", jstr(&request.system))])]))]),
        );
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
            &mut inner,
            "tools",
            arr([obj([("functionDeclarations", arr(decls))])]),
        );
    }
    let mut gen_config = obj([]);
    if let Some(max_tokens) = request.max_tokens {
        gen_config["maxOutputTokens"] = max_tokens.into();
    }
    if let Some(temp) = request.temperature {
        crate::openai_compat::set_json_f64(&mut gen_config, "temperature", temp);
    }
    if !gen_config.as_object().is_none_or(|o| o.is_empty()) {
        inner["generationConfig"] = gen_config;
    }
    inner
}

fn apply_antigravity_inner(inner: &mut Value, request: &LlmRequest) {
    if let Some(si) = inner.get_mut("systemInstruction") {
        json_value::insert(si, "role", jstr("user"));
    }
    if !request.tools.is_empty() {
        json_value::insert(
            inner,
            "toolConfig",
            obj([("functionCallingConfig", obj([("mode", jstr("VALIDATED"))]))]),
        );
    }
}

fn antigravity_ids() -> (String, String, String) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(1);
    let agent = format!("{now:016x}");
    let traj = format!("{:016x}", now.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 8);
    let request_id = format!("agent/{agent}/{now}/{traj}/2");
    let session = format!("-{}", now & ((1u128 << 63) - 1));
    (session, request_id, traj)
}

fn wrap_envelope(
    profile: &Profile,
    model: &str,
    project: &str,
    mut inner: Value,
    request: &LlmRequest,
) -> Value {
    if profile.antigravity {
        apply_antigravity_inner(&mut inner, request);
        let (session, request_id, traj) = antigravity_ids();
        let claude = model.to_ascii_lowercase().contains("claude");
        json_value::insert(&mut inner, "sessionId", jstr(session));
        json_value::insert(
            &mut inner,
            "labels",
            obj([
                ("last_step_index", jstr("1")),
                ("trajectory_id", jstr(traj)),
                ("used_claude", jstr(claude.to_string())),
                ("used_claude_conservative", jstr(claude.to_string())),
            ]),
        );
        obj([
            ("model", jstr(model)),
            ("project", jstr(project)),
            ("request", inner),
            ("userAgent", jstr("antigravity")),
            ("requestType", jstr("agent")),
            ("requestId", jstr(request_id)),
        ])
    } else {
        obj([
            ("model", jstr(model)),
            ("project", jstr(project)),
            ("request", inner),
        ])
    }
}

fn code_assist_http_error(
    status: impl std::fmt::Display,
    model: &str,
    json: &Value,
    raw_body: &str,
) -> String {
    let from_field = json
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("");
    let reason = crate::error_class::extract_provider_reason(from_field)
        .or_else(|| crate::error_class::extract_provider_reason(raw_body))
        .or_else(|| {
            let t = from_field.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        })
        .unwrap_or_else(|| {
            let t = raw_body.trim();
            if t.is_empty() {
                "Unknown error".into()
            } else {
                t.chars().take(240).collect()
            }
        });
    format!("Code Assist error ({status}) model={model}: {reason}")
}

/// Map one Code Assist SSE chunk (a GenerateContentResponse, tolerating a
/// `{"response": …}` wrapper) to whycodes stream events. Pure for tests.
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
                    if let Some(sig) = part["thoughtSignature"].as_str()
                        && !sig.is_empty()
                    {
                        events.push(StreamEvent::ThinkingSignature {
                            signature: sig.to_string(),
                        });
                    }
                    let thought = part["thought"].as_bool().unwrap_or(false);
                    if thought {
                        if let Some(text) = part["text"].as_str()
                            && !text.is_empty()
                        {
                            events.push(StreamEvent::Thinking {
                                text: text.to_string(),
                            });
                        }
                        continue;
                    }
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
                    delta: obj([("finishReason", jstr(reason))]),
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
) -> whycodes_core::Result<LlmResponse> {
    complete_with(&gemini_cli_profile(), request, api_key, model).await
}

/// Antigravity subscription path (`google-antigravity` OAuth).
pub async fn complete_antigravity(
    request: &LlmRequest,
    api_key: &str,
    model: &str,
) -> whycodes_core::Result<LlmResponse> {
    complete_with(&antigravity_profile(), request, api_key, model).await
}

async fn complete_with(
    profile: &Profile,
    request: &LlmRequest,
    api_key: &str,
    model: &str,
) -> whycodes_core::Result<LlmResponse> {
    let project = project_id(profile, api_key).await?;
    let model = if profile.antigravity {
        antigravity_wire_model(model)
    } else {
        model
    };
    let body = wrap_envelope(
        profile,
        model,
        &project,
        build_inner_request(request),
        request,
    );
    let resp = post_generate(profile, ":generateContent", api_key, &body).await?;
    let status = resp.status();
    let json: Value = resp
        .json()
        .await
        .map_err(|e| whycodes_core::Error::llm(format!("Code Assist parse: {e}")))?;
    if !status.is_success() {
        return Err(whycodes_core::Error::llm(code_assist_http_error(
            status,
            model,
            &json,
            &json.to_string(),
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
) -> whycodes_core::Result<Pin<Box<dyn Stream<Item = whycodes_core::Result<StreamEvent>> + Send>>> {
    stream_with(&gemini_cli_profile(), request, api_key, model).await
}

/// Antigravity subscription path (`google-antigravity` OAuth).
pub async fn stream_antigravity(
    request: &LlmRequest,
    api_key: &str,
    model: &str,
) -> whycodes_core::Result<Pin<Box<dyn Stream<Item = whycodes_core::Result<StreamEvent>> + Send>>> {
    stream_with(&antigravity_profile(), request, api_key, model).await
}

async fn stream_with(
    profile: &Profile,
    request: &LlmRequest,
    api_key: &str,
    model: &str,
) -> whycodes_core::Result<Pin<Box<dyn Stream<Item = whycodes_core::Result<StreamEvent>> + Send>>> {
    let project = project_id(profile, api_key).await?;
    let model = if profile.antigravity {
        antigravity_wire_model(model)
    } else {
        model
    };
    let body = wrap_envelope(
        profile,
        model,
        &project,
        build_inner_request(request),
        request,
    );
    let resp = post_generate(profile, ":streamGenerateContent?alt=sse", api_key, &body).await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let json: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        return Err(whycodes_core::Error::llm(code_assist_http_error(
            status, model, &json, &text,
        )));
    }

    Ok(Box::pin(CodeAssistSse {
        bytes: crate::openai_compat::response_bytes(resp),
        buffer: String::new(),
        pending: VecDeque::new(),
        call_seq: 0,
        done: false,
    }) as ProviderEventStream)
}

struct CodeAssistSse {
    bytes: crate::openai_compat::ByteStream,
    buffer: String,
    pending: VecDeque<whycodes_core::Result<StreamEvent>>,
    call_seq: u64,
    done: bool,
}

impl CodeAssistSse {
    fn push_data_line(&mut self, line: &str) {
        if line.is_empty() || !line.starts_with("data: ") {
            return;
        }
        for ev in events_for_chunk(&line[6..], &mut self.call_seq) {
            if matches!(ev, StreamEvent::MessageStop) {
                self.done = true;
            }
            self.pending.push_back(Ok(ev));
        }
    }
}

impl Stream for CodeAssistSse {
    type Item = whycodes_core::Result<StreamEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(ev) = this.pending.pop_front() {
                return Poll::Ready(Some(ev));
            }
            if this.done {
                return Poll::Ready(None);
            }
            match this.bytes.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    this.buffer.push_str(&String::from_utf8_lossy(&bytes));
                    while let Some(pos) = this.buffer.find('\n') {
                        let line = this.buffer[..pos].trim().to_string();
                        this.buffer = this.buffer[pos + 1..].to_string();
                        this.push_data_line(&line);
                        if this.done {
                            break;
                        }
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(crate::openai_compat::stream_chunk_error(
                        "codeassist",
                        e,
                    ))));
                }
                Poll::Ready(None) => {
                    if !this.done {
                        this.pending.push_back(Ok(StreamEvent::MessageStop));
                        this.done = true;
                    }
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[cfg(test)]
#[path = "codeassist_tests.rs"]
mod tests;
