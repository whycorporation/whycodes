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

    // 2. Not onboarded: run onboardUser on the allowed tier and poll the
    // long-running operation until it yields a project id. Provisioning can
    // take a couple of minutes (project creation server-side), so poll well
    // past a few seconds — bailing early surfaces as "no project id".
    let tier = pick_tier(&load);
    let mut body = json!({ "tierId": tier, "metadata": client_metadata() });
    // Tiers flagged `userDefinedCloudaicompanionProject` (standard-tier for
    // Pro accounts) expect the caller's own Cloud project in this field —
    // same shape Gemini CLI sends. Harmless when absent.
    if let Some(project) = explicit_project(&token) {
        body["cloudaicompanionProject"] = Value::String(project);
    }
    let onboard = post(":onboardUser", &token.access_token, &body).await?;
    let mut operation = onboard;
    for _ in 0..60 {
        if operation["done"].as_bool().unwrap_or(false) {
            break;
        }
        let Some(name) = operation["name"].as_str().map(str::to_string) else {
            break;
        };
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        operation = post(&format!("/{name}"), &token.access_token, &json!({})).await?;
    }

    match yielded_project(&operation) {
        Some(id) => {
            token
                .extra
                .insert(PROJECT_ID_KEY.to_string(), Value::String(id));
            Ok(token)
        }
        None => {
            let done = operation["done"].as_bool().unwrap_or(false);
            Err(AuthError::Provider(format!(
                "Code Assist onboarding did not yield a project id \
                 (operation done={done}, tier={tier}) — set GOOGLE_CLOUD_PROJECT \
                 to your Cloud project id and retry"
            )))
        }
    }
}

/// Pull the provisioned project id out of a finished `onboardUser` LRO.
/// The service has shipped both shapes: an object (`{"id": …}`) and a bare
/// project-id string, so accept either.
fn yielded_project(operation: &Value) -> Option<String> {
    let proj = &operation["response"]["cloudaicompanionProject"];
    if let Some(id) = proj["id"].as_str().filter(|s| !s.is_empty()) {
        return Some(id.to_string());
    }
    proj.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

/// The tier to pass to `onboardUser`, from the `allowedTiers` the account
/// reported in its `loadCodeAssist` response.
///
/// A Pro/paid account's tier list only carries `"standard-tier"` (which
/// accepts a user-defined `cloudaicompanionProject`); forcing `"free-tier"`
/// makes `onboardUser` reject the call with 403. Prefer the paid tier when
/// present, and only fall back to the managed-project (free) tier otherwise.
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
    // Pro customers cannot onboard into the free tier; pick the one that
    // takes a user-defined project when the account reports it.
    find(true).unwrap_or_else(|| find(false).unwrap_or_else(|| "free-tier".to_string()))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_tier_prefers_user_defined_project() {
        let load = json!({"allowedTiers": [
            {"id": "free-tier", "userDefinedCloudaicompanionProject": false},
            {"id": "standard-tier", "userDefinedCloudaicompanionProject": true}
        ]});
        assert_eq!(pick_tier(&load), "standard-tier");
    }

    #[test]
    fn pick_tier_falls_back_to_free_tier() {
        let free = json!({"allowedTiers": [
            {"id": "free-tier", "userDefinedCloudaicompanionProject": false}
        ]});
        assert_eq!(pick_tier(&free), "free-tier");

        let empty = json!({});
        assert_eq!(pick_tier(&empty), "free-tier");
    }

    #[test]
    fn yielded_project_reads_object_and_string_shapes() {
        let object = json!({
            "done": true,
            "response": {"cloudaicompanionProject": {"id": "gen-lang-client-123"}}
        });
        assert_eq!(
            yielded_project(&object).as_deref(),
            Some("gen-lang-client-123")
        );

        let plain = json!({
            "done": true,
            "response": {"cloudaicompanionProject": "my-project-456"}
        });
        assert_eq!(yielded_project(&plain).as_deref(), Some("my-project-456"));

        let pending = json!({"done": false});
        assert_eq!(yielded_project(&pending), None);
    }
}
