//! Cloud Code Assist (CCA) transport logic for Antigravity integration.
//!
//! Handles onboarding a fresh Google OAuth token onto the Code Assist
//! control plane (`cloudcode-pa.googleapis.com`) so subsequent chat calls
//! resolve a Cloud project. The wire schema mirrors what Gemini CLI sends:
//! camelCase fields, `cloudaicompanionProject` for the project handle, and a
//! `metadata` object carrying `ideType` / `platform` / `pluginType`.
//!
//! The resolved project id is stored back on the token under
//! `extra["project_id"]` (the same canonical key the LLM provider reads), so
//! the auth step and the chat provider agree on the account's project.

use crate::error::{AuthError, Result};
use crate::token::OAuthToken;
use serde_json::{Value, json};

const BASE: &str = "https://cloudcode-pa.googleapis.com/v1internal";

/// Canonical `extra` key used to persist a resolved Google Cloud project id.
pub const PROJECT_ID_KEY: &str = "project_id";

/// Client metadata the Code Assist service expects (Gemini CLI sends the
/// same shape; the values identify an Antigravity IDE).
fn client_metadata() -> Value {
    json!({
        "ideType": "ANTIGRAVITY",
        "platform": "PLATFORM_UNSPECIFIED",
        "pluginType": "GEMINI",
    })
}

/// POST a JSON body to the Code Assist control plane and decode the JSON
/// response. Non-success statuses surface the provider's `error.message`
/// body rather than a bare "400 Bad Request".
async fn post(suffix: &str, token: &str, body: &Value) -> Result<Value> {
    let url = format!("{BASE}{suffix}");
    let resp = reqwest::Client::new()
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .json(body)
        .send()
        .await
        .map_err(AuthError::from)?;
    let status = resp.status();
    let value: Value = resp.json().await.map_err(AuthError::from)?;
    if !status.is_success() {
        // The Code Assist control plane surfaces the actionable reason under
        // `error.message`, but its "Your account is not eligible …" copy can
        // be truncated (or replaced with a bare "unknown error") when the
        // body shape differs. Favor `error.message`, then the message-only
        // variant, and only then fall back to a keyword so the user still
        // learns why sign-in was refused.
        let msg = value["error"]["message"]
            .as_str()
            .or_else(|| value["message"].as_str())
            .or_else(|| value["error"].as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("unknown error");
        // Detect the free-tier ineligibility reason even when the provider is
        // terse, so the auth failure matches whycode-llm's actionable copy.
        if status.as_u16() == 403
            && (msg.to_ascii_lowercase().contains("eligib")
                || msg.eq_ignore_ascii_case("unknown error"))
        {
            return Err(AuthError::Provider(format!(
                "Code Assist request (`{suffix}`) failed ({status}): your Google account is not eligible for Gemini Code Assist for individuals (free tier) — use an AI Studio API key (GOOGLE_API_KEY) or a different account"
            )));
        }
        return Err(AuthError::Provider(format!(
            "Code Assist request (`{suffix}`) failed ({status}): {msg}"
        )));
    }
    Ok(value)
}

/// Attach a Cloud project to `token` by discovering or creating one on the
/// Code Assist control plane.
///
/// Resolution order matches `whycode-llm`'s Code Assist provider: an explicit
/// `token.extra[PROJECT_ID_KEY]` (or `GOOGLE_CLOUD_PROJECT`) is used as-is;
/// otherwise `loadCodeAssist` returns the managed project of an
/// already-onboarded account, and a fallback runs `onboardUser` on the tier
/// the account is allowed, polling the long-running operation until it
/// yields a project id. On success the project id is stored on the token.
pub async fn perform_antigravity_onboarding(mut token: OAuthToken) -> Result<OAuthToken> {
    if let Some(project) = explicit_project(&token) {
        token
            .extra
            .insert(PROJECT_ID_KEY.to_string(), Value::String(project));
        return Ok(token);
    }

    // 1. Already onboarded? loadCodeAssist reports the managed project (or
    // the tiers the account may onboard into).
    let load = post(
        ":loadCodeAssist",
        &token.access_token,
        &json!({ "metadata": client_metadata() }),
    )
    .await?;
    if let Some(id) = load["cloudaicompanionProject"].as_str() {
        token
            .extra
            .insert(PROJECT_ID_KEY.to_string(), Value::String(id.to_string()));
        return Ok(token);
    }

    // 2. Not onboarded: pick the free/managed tier and run onboardUser,
    // polling the long-running operation until it yields a project id.
    let tier = pick_tier(&load);
    let onboard = post(
        ":onboardUser",
        &token.access_token,
        &json!({ "tierId": tier, "metadata": client_metadata() }),
    )
    .await?;
    let mut operation = onboard;
    for _ in 0..10 {
        if operation["done"].as_bool().unwrap_or(false) {
            break;
        }
        let Some(name) = operation["name"].as_str().map(str::to_string) else {
            break;
        };
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        operation = post(&format!("/{name}"), &token.access_token, &json!({})).await?;
    }

    match operation["response"]["cloudaicompanionProject"]["id"].as_str() {
        Some(id) => {
            token
                .extra
                .insert(PROJECT_ID_KEY.to_string(), Value::String(id.to_string()));
            Ok(token)
        }
        None => Err(AuthError::Provider(
            "Code Assist onboarding did not yield a project id".into(),
        )),
    }
}

/// The tier to pass to `onboardUser`, from the `allowedTiers` the account
/// reported in its `loadCodeAssist` response. Falls back to the free
/// (managed-project) tier when the list is absent.
fn pick_tier(load_response: &Value) -> String {
    let find = |want_user_project: bool| {
        load_response["allowedTiers"].as_array().and_then(|tiers| {
            tiers
                .iter()
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
    find(false).unwrap_or_else(|| "free-tier".to_string())
}

/// A caller-supplied project: the stored token extra first, then the env.
fn explicit_project(token: &OAuthToken) -> Option<String> {
    token
        .extra
        .get(PROJECT_ID_KEY)
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .or_else(|| {
            std::env::var("GOOGLE_CLOUD_PROJECT")
                .ok()
                .filter(|p| !p.is_empty())
        })
}
