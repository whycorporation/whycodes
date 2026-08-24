//! Cloud Code Assist (CCA) transport logic for Antigravity integration.
//!
//! Handles onboarding a fresh Google OAuth token onto the Code Assist
//! control plane (`daily-cloudcode-pa.googleapis.com`) so subsequent chat
//! calls resolve a Cloud project. The wire schema mirrors native Antigravity:
//! camelCase fields, `cloudaicompanionProject` for the project handle, and a
//! `metadata` object carrying `ideType: ANTIGRAVITY`.
//!
//! The resolved project id is stored back on the token under
//! `extra["project_id"]` (the same canonical key the LLM provider reads), so
//! the auth step and the chat provider agree on the account's project.

use crate::error::{AuthError, Result};
use crate::token::OAuthToken;
use serde_json::{Value, json};

const BASE: &str = "https://daily-cloudcode-pa.googleapis.com/v1internal";

/// User-Agent identifying as the native Antigravity client (captured from the
/// real 2.8.0 `antigravity/hub` release). The backend gates the control plane
/// on this header.
const ANTIGRAVITY_USER_AGENT: &str =
    "antigravity/hub/2.8.0 (aidev_client; os_type=linux; arch=x86_64; cl=963137146)";

/// Canonical `extra` key used to persist a resolved Google Cloud project id.
pub const PROJECT_ID_KEY: &str = "project_id";

/// Client metadata the Code Assist service expects. The native Antigravity
/// client sends only `ideType` — the extra `platform`/`pluginType` keys from
/// the Gemini CLI provider are not part of this control plane's schema.
fn client_metadata() -> Value {
    json!({ "ideType": "ANTIGRAVITY" })
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
        .header("User-Agent", ANTIGRAVITY_USER_AGENT)
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
/// Resolution order mirrors the native Antigravity control-plane flow: an
/// explicit `token.extra[PROJECT_ID_KEY]` (or `GOOGLE_CLOUD_PROJECT`) is used
/// as-is; otherwise `loadCodeAssist` reports the project of an
/// already-onboarded account — accounts with a bound tier skip provisioning
/// entirely. Only fresh accounts run `onboardUser` (on the tier they are
/// allowed), and the project is read back from a **fresh** `loadCodeAssist`,
/// because the finished long-running operation frequently ships no project in
/// its response body.
pub async fn perform_antigravity_onboarding(mut token: OAuthToken) -> Result<OAuthToken> {
    if let Some(project) = explicit_project(&token) {
        token
            .extra
            .insert(PROJECT_ID_KEY.to_string(), Value::String(project));
        return Ok(token);
    }

    // 1. Account status (with the bind-and-confirm double call).
    let load = load_code_assist(&token.access_token).await?;

    // 2. Provisioning is only for accounts without a bound tier yet; calling
    //    `onboardUser` on an already-onboarded (e.g. paid) account fails.
    let mut operation: Option<Value> = None;
    if load.get("currentTier").is_none_or(Value::is_null) {
        surface_free_tier_ineligibility(&load)?;
        let tier = pick_tier(&load);
        let mut body = json!({ "tierId": tier, "metadata": client_metadata() });
        // Tiers flagged `userDefinedCloudaicompanionProject` expect the
        // caller's own Cloud project in this field — same shape Gemini CLI
        // sends. Harmless when absent.
        if let Some(project) = explicit_project(&token) {
            body["cloudaicompanionProject"] = Value::String(project);
        }
        let onboard = post(":onboardUser", &token.access_token, &body).await?;
        let mut current = onboard;
        for _ in 0..60 {
            if current["done"].as_bool().unwrap_or(false) {
                break;
            }
            let Some(name) = current["name"].as_str().map(str::to_string) else {
                break;
            };
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            current = post(&format!("/{name}"), &token.access_token, &json!({})).await?;
        }
        if current["done"].as_bool().unwrap_or(false)
            && let Some(err) = current["error"]["message"].as_str()
        {
            return Err(AuthError::Provider(format!(
                "Code Assist onboarding operation failed: {err}"
            )));
        }
        operation = Some(current);
    }

    // 3. Resolve the project: the authoritative fresh load first, then the
    //    finished operation's body, and only then give up with guidance.
    let refreshed = load_code_assist(&token.access_token).await?;
    let resolved =
        load_project(&refreshed).or_else(|| operation.as_ref().and_then(yielded_project));
    match resolved {
        Some(id) => {
            token
                .extra
                .insert(PROJECT_ID_KEY.to_string(), Value::String(id));
            Ok(token)
        }
        None => Err(AuthError::Provider(
            "Code Assist onboarding did not yield a project id \
             (loadCodeAssist returned no cloudaicompanionProject) — set \
             GOOGLE_CLOUD_PROJECT to your Cloud project id and retry"
                .into(),
        )),
    }
}

/// POST `:loadCodeAssist`. When the first response reports a project without a
/// paid-tier binding, repeat the call WITH that project handle (native
/// Antigravity behavior) so the service binds it before reporting state.
async fn load_code_assist(token: &str) -> Result<Value> {
    let mut load = post(
        ":loadCodeAssist",
        token,
        &json!({ "metadata": client_metadata() }),
    )
    .await?;
    if let Some(project) = load_project(&load)
        && load.get("paidTier").is_none_or(Value::is_null)
    {
        load = post(
            ":loadCodeAssist",
            token,
            &json!({
                "cloudaicompanionProject": project,
                "metadata": client_metadata(),
            }),
        )
        .await?;
    }
    Ok(load)
}

/// The plain-string project handle from a `loadCodeAssist` payload (the field
/// ships as a bare string, not an object).
fn load_project(payload: &Value) -> Option<String> {
    payload["cloudaicompanionProject"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Surface Google's own reason when the account cannot onboard.
///
/// After Gemini Code Assist for individuals shut down (2026-06-18), Antigravity
/// tokens often report `free-tier` as ineligible with "This client is no longer
/// supported for Gemini Code Assist". That is expected and **not** fatal when
/// another tier is in `allowedTiers` — `pick_tier` will onboard onto that one.
/// Only fail when there is no allowed tier at all.
fn surface_free_tier_ineligibility(load: &Value) -> Result<()> {
    let has_allowed_tier = load["allowedTiers"].as_array().is_some_and(|tiers| {
        tiers
            .iter()
            .any(|t| t["id"].as_str().is_some_and(|id| !id.is_empty()))
    });
    if has_allowed_tier {
        return Ok(());
    }
    let free_entry = || {
        load["ineligibleTiers"].as_array().and_then(|tiers| {
            tiers
                .iter()
                .find(|t| t["tierId"].as_str() == Some("free-tier"))
        })
    };
    if let Some(reason) = free_entry().and_then(|t| t["reasonMessage"].as_str()) {
        let url = free_entry()
            .and_then(|t| t["validationUrl"].as_str())
            .map(|u| format!("\n{u}"))
            .unwrap_or_default();
        return Err(AuthError::Provider(format!(
            "Google Code Assist: {reason}{url}"
        )));
    }
    Ok(())
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

    #[test]
    fn load_project_reads_bare_string_field() {
        let payload = json!({"cloudaicompanionProject": "proj-789"});
        assert_eq!(load_project(&payload).as_deref(), Some("proj-789"));

        let absent = json!({});
        assert_eq!(load_project(&absent), None);

        let empty = json!({"cloudaicompanionProject": ""});
        assert_eq!(load_project(&empty), None);
    }

    #[test]
    fn ineligibility_reason_is_surfaced() {
        // A paid/Antigravity tier in allowedTiers means free-tier ineligibility
        // is expected after the Gemini Code Assist consumer sunset — keep going.
        let paid_allowed = json!({
            "allowedTiers": [{"id": "standard-tier"}],
            "ineligibleTiers": [{
                "tierId": "free-tier",
                "reasonMessage": "This client is no longer supported for Gemini Code Assist",
                "validationUrl": "https://example.com/fix"
            }]
        });
        assert!(surface_free_tier_ineligibility(&paid_allowed).is_ok());

        let ineligible = json!({
            "allowedTiers": [],
            "ineligibleTiers": [{
                "tierId": "free-tier",
                "reasonMessage": "not eligible for individuals",
                "validationUrl": "https://example.com/fix"
            }]
        });
        let err = surface_free_tier_ineligibility(&ineligible)
            .expect_err("should surface Google's reason");
        assert!(err.to_string().contains("not eligible for individuals"));
        assert!(err.to_string().contains("https://example.com/fix"));

        let eligible = json!({"allowedTiers": [
            {"id": "free-tier"}, {"id": "standard-tier"}
        ]});
        assert!(surface_free_tier_ineligibility(&eligible).is_ok());

        let silent = json!({});
        assert!(surface_free_tier_ineligibility(&silent).is_ok());
    }
}
