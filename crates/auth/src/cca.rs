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
use crate::spec::inference_identity;
use crate::token::OAuthToken;
use serde_json::{Value, json};

const BASE: &str = "https://daily-cloudcode-pa.googleapis.com/v1internal";

const ONBOARD_POLL_DELAY: std::time::Duration = onboard_poll_delay(cfg!(test));

const fn onboard_poll_delay(for_test: bool) -> std::time::Duration {
    if for_test {
        std::time::Duration::from_millis(1)
    } else {
        std::time::Duration::from_secs(3)
    }
}

#[cfg(test)]
fn test_base() -> &'static std::sync::Mutex<Option<String>> {
    static BASE: std::sync::LazyLock<std::sync::Mutex<Option<String>>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(None));
    &BASE
}

fn control_plane_url(suffix: &str) -> String {
    #[cfg(test)]
    if let Some(base) = test_base()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        return format!("{base}{suffix}");
    }
    format!("{BASE}{suffix}")
}

/// Canonical `extra` key used to persist a resolved Google Cloud project id.
pub const PROJECT_ID_KEY: &str = "project_id";

fn request_user_agent() -> String {
    inference_identity("google-antigravity")
        .and_then(|id| id.user_agent)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| concat!("whycodes/", env!("CARGO_PKG_VERSION")).to_string())
}

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
    send(reqwest::Method::POST, suffix, token, Some(body)).await
}

/// GET an LRO status. Native Antigravity / oh-my-pi poll `:onboardUser`
/// operations with GET, not POST.
async fn get(suffix: &str, token: &str) -> Result<Value> {
    send(reqwest::Method::GET, suffix, token, None).await
}

async fn send(
    method: reqwest::Method,
    suffix: &str,
    token: &str,
    body: Option<&Value>,
) -> Result<Value> {
    let url = control_plane_url(suffix);
    let mut req = reqwest::Client::new()
        .request(method, &url)
        .header("Authorization", format!("Bearer {token}"))
        .header("User-Agent", request_user_agent());
    if let Some(body) = body {
        req = req.header("Content-Type", "application/json").json(body);
    }
    let resp = req.send().await.map_err(AuthError::from)?;
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
        // terse, so the auth failure matches whycodes-llm's actionable copy.
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
        if let Some(project) = explicit_project(&token).or_else(|| test_onboard_project(&token)) {
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
            tokio::time::sleep(ONBOARD_POLL_DELAY).await;
            current = get(&format!("/{name}"), &token.access_token).await?;
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

fn test_onboard_project(token: &OAuthToken) -> Option<String> {
    token
        .extra
        .get("onboard_project")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
pub(crate) mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Mutex, MutexGuard};
    use std::thread;

    #[test]
    fn user_agent_is_whycodes_without_plugin() {
        let _lock = env_lock();
        crate::spec::lock_registry().remove("google-antigravity");
        let ua = request_user_agent();
        assert!(
            ua.starts_with("whycodes/"),
            "core CCA traffic must not impersonate Antigravity without a plugin: {ua}"
        );
    }

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

    pub(crate) fn cca_test_lock() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn env_lock() -> MutexGuard<'static, ()> {
        cca_test_lock()
    }

    struct EnvRestore {
        project: Option<std::ffi::OsString>,
    }

    impl EnvRestore {
        fn capture() -> Self {
            Self {
                project: std::env::var_os("GOOGLE_CLOUD_PROJECT"),
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match self.project.take() {
                Some(v) => unsafe { std::env::set_var("GOOGLE_CLOUD_PROJECT", v) },
                None => unsafe { std::env::remove_var("GOOGLE_CLOUD_PROJECT") },
            }
        }
    }

    struct TestBaseGuard;

    impl Drop for TestBaseGuard {
        fn drop(&mut self) {
            *test_base().lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }

    fn set_test_base(base: String) -> TestBaseGuard {
        *test_base().lock().unwrap_or_else(|e| e.into_inner()) = Some(base);
        TestBaseGuard
    }

    fn oauth_token() -> OAuthToken {
        OAuthToken {
            access_token: "ya29.test".into(),
            refresh_token: None,
            expires_at: None,
            extra: Default::default(),
        }
    }

    fn mock_cca(responses: Vec<(u16, String)>) -> String {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                loop {
                    line.clear();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                }
                let reason = if status == 200 { "OK" } else { "Error" };
                write!(
                    stream,
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        format!("http://{addr}/v1internal")
    }

    fn json_body(value: Value) -> String {
        value.to_string()
    }

    #[tokio::test]
    async fn onboarding_uses_explicit_token_project() {
        let mut token = oauth_token();
        token
            .extra
            .insert(PROJECT_ID_KEY.into(), Value::String("from-extra".into()));
        let out = perform_antigravity_onboarding(token).await.unwrap();
        assert_eq!(out.extra[PROJECT_ID_KEY], "from-extra");
    }

    #[tokio::test]
    async fn onboarding_uses_env_project_when_extra_empty() {
        let _lock = env_lock();
        let _restore = EnvRestore::capture();
        unsafe { std::env::set_var("GOOGLE_CLOUD_PROJECT", "from-env") };
        let mut token = oauth_token();
        token
            .extra
            .insert(PROJECT_ID_KEY.into(), Value::String(String::new()));
        let out = perform_antigravity_onboarding(token).await.unwrap();
        assert_eq!(out.extra[PROJECT_ID_KEY], "from-env");
    }

    #[test]
    fn explicit_project_ignores_empty_env() {
        let _lock = env_lock();
        let _restore = EnvRestore::capture();
        unsafe { std::env::set_var("GOOGLE_CLOUD_PROJECT", "") };
        assert!(explicit_project(&oauth_token()).is_none());
    }

    #[test]
    fn user_agent_uses_plugin_identity_when_set() {
        let _lock = env_lock();
        crate::spec::register_spec(crate::spec::ProviderSpec {
            name: "google-antigravity".into(),
            label: "Antigravity".into(),
            flow: crate::spec::FlowKind::LoopbackPkce,
            client_id: "cid".into(),
            client_secret: None,
            authorize_url: "https://example.com/auth".into(),
            token_url: "https://example.com/token".into(),
            scopes: "read".into(),
            token_encoding: crate::spec::TokenEncoding::Form,
            redirect_uri: None,
            loopback_port: None,
            loopback_host: None,
            callback_path: "/cb".into(),
            extra_authorize: vec![],
            derived: None,
            suggested_models: vec![],
            inference: Some(crate::spec::InferenceIdentity {
                user_agent: Some("antigravity-ua".into()),
                headers: Default::default(),
            }),
        });
        assert_eq!(request_user_agent(), "antigravity-ua");
        crate::spec::register_spec(crate::spec::ProviderSpec {
            name: "google-antigravity".into(),
            label: "Antigravity".into(),
            flow: crate::spec::FlowKind::LoopbackPkce,
            client_id: "cid".into(),
            client_secret: None,
            authorize_url: "https://example.com/auth".into(),
            token_url: "https://example.com/token".into(),
            scopes: "read".into(),
            token_encoding: crate::spec::TokenEncoding::Form,
            redirect_uri: None,
            loopback_port: None,
            loopback_host: None,
            callback_path: "/cb".into(),
            extra_authorize: vec![],
            derived: None,
            suggested_models: vec![],
            inference: Some(crate::spec::InferenceIdentity {
                user_agent: Some(String::new()),
                headers: Default::default(),
            }),
        });
        assert!(request_user_agent().starts_with("whycodes/"));
        crate::spec::lock_registry().remove("google-antigravity");
    }

    #[tokio::test]
    async fn onboarding_skips_provisioning_when_current_tier_is_bound() {
        let _lock = env_lock();
        let _restore = EnvRestore::capture();
        unsafe { std::env::remove_var("GOOGLE_CLOUD_PROJECT") };
        let _base = set_test_base(mock_cca(vec![
            (
                200,
                json_body(json!({
                    "currentTier": {"id": "standard-tier"},
                    "cloudaicompanionProject": "bound-proj"
                })),
            ),
            (
                200,
                json_body(json!({
                    "currentTier": {"id": "standard-tier"},
                    "paidTier": {"id": "standard-tier"},
                    "cloudaicompanionProject": "bound-proj"
                })),
            ),
            (
                200,
                json_body(json!({
                    "currentTier": {"id": "standard-tier"},
                    "paidTier": {"id": "standard-tier"},
                    "cloudaicompanionProject": "bound-proj"
                })),
            ),
        ]));
        let out = perform_antigravity_onboarding(oauth_token()).await.unwrap();
        assert_eq!(out.extra[PROJECT_ID_KEY], "bound-proj");
    }

    #[tokio::test]
    async fn onboarding_polls_until_done_and_reads_operation_project() {
        let _lock = env_lock();
        let _restore = EnvRestore::capture();
        unsafe { std::env::remove_var("GOOGLE_CLOUD_PROJECT") };
        let _base = set_test_base(mock_cca(vec![
            (
                200,
                json_body(json!({
                    "allowedTiers": [{"id": "free-tier"}]
                })),
            ),
            (
                200,
                json_body(json!({
                    "name": "operations/op-1",
                    "done": false
                })),
            ),
            (
                200,
                json_body(json!({
                    "done": true,
                    "response": {"cloudaicompanionProject": {"id": "from-lro"}}
                })),
            ),
            (200, json_body(json!({}))),
        ]));
        let mut token = oauth_token();
        token
            .extra
            .insert("onboard_project".into(), Value::String("hint-proj".into()));
        let out = perform_antigravity_onboarding(token).await.unwrap();
        assert_eq!(out.extra[PROJECT_ID_KEY], "from-lro");
    }

    #[tokio::test]
    async fn onboarding_breaks_poll_without_operation_name() {
        let _lock = env_lock();
        let _restore = EnvRestore::capture();
        unsafe { std::env::remove_var("GOOGLE_CLOUD_PROJECT") };
        let _base = set_test_base(mock_cca(vec![
            (
                200,
                json_body(json!({"allowedTiers": [{"id": "free-tier"}]})),
            ),
            (200, json_body(json!({"done": false}))),
            (200, json_body(json!({}))),
        ]));
        let err = perform_antigravity_onboarding(oauth_token())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("did not yield a project id"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn onboarding_surfaces_finished_operation_error() {
        let _lock = env_lock();
        let _restore = EnvRestore::capture();
        unsafe { std::env::remove_var("GOOGLE_CLOUD_PROJECT") };
        let _base = set_test_base(mock_cca(vec![
            (
                200,
                json_body(json!({"allowedTiers": [{"id": "free-tier"}]})),
            ),
            (
                200,
                json_body(json!({
                    "done": true,
                    "error": {"message": "quota exceeded"}
                })),
            ),
        ]));
        let err = perform_antigravity_onboarding(oauth_token())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("quota exceeded"), "{err}");
    }

    #[tokio::test]
    async fn onboarding_surfaces_ineligibility_before_onboard() {
        let _lock = env_lock();
        let _restore = EnvRestore::capture();
        unsafe { std::env::remove_var("GOOGLE_CLOUD_PROJECT") };
        let _base = set_test_base(mock_cca(vec![(
            200,
            json_body(json!({
                "allowedTiers": [],
                "ineligibleTiers": [{
                    "tierId": "free-tier",
                    "reasonMessage": "not eligible for individuals"
                }]
            })),
        )]));
        let err = perform_antigravity_onboarding(oauth_token())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("not eligible for individuals"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn send_maps_error_message_shapes() {
        let _lock = env_lock();
        let _base = set_test_base(mock_cca(vec![
            (
                403,
                json_body(json!({"error": {"message": "not eligible"}})),
            ),
            (403, json_body(json!({}))),
            (400, json_body(json!({"message": "bad request"}))),
            (400, json_body(json!({"error": "plain"}))),
            (400, json_body(json!({"error": {"message": ""}}))),
        ]));
        let err = post(":loadCodeAssist", "tok", &json!({}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not eligible"), "{err}");
        let err = get("/operations/x", "tok").await.unwrap_err();
        assert!(err.to_string().contains("not eligible"), "{err}");
        let err = post(":loadCodeAssist", "tok", &json!({}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("bad request"), "{err}");
        let err = post(":loadCodeAssist", "tok", &json!({}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("plain"), "{err}");
        let err = post(":loadCodeAssist", "tok", &json!({}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unknown error"), "{err}");
    }

    #[test]
    fn control_plane_url_uses_production_base_without_override() {
        let _lock = env_lock();
        *test_base().lock().unwrap_or_else(|e| e.into_inner()) = None;
        assert_eq!(
            control_plane_url(":loadCodeAssist"),
            format!("{BASE}:loadCodeAssist")
        );
    }

    #[test]
    fn test_onboard_project_reads_hint() {
        let mut token = oauth_token();
        assert!(test_onboard_project(&token).is_none());
        token
            .extra
            .insert("onboard_project".into(), Value::String("hint".into()));
        assert_eq!(test_onboard_project(&token).as_deref(), Some("hint"));
    }

    #[test]
    fn onboard_poll_delay_covers_test_and_production() {
        assert_eq!(
            onboard_poll_delay(true),
            std::time::Duration::from_millis(1)
        );
        assert_eq!(onboard_poll_delay(false), std::time::Duration::from_secs(3));
    }

    #[test]
    fn test_base_recovers_from_poison() {
        let _lock = env_lock();
        let handle = std::thread::spawn(|| {
            let _guard = test_base().lock().unwrap();
            panic!("poison the cca test base");
        });
        let _ = handle.join();
        *test_base().lock().unwrap_or_else(|e| e.into_inner()) = None;
        assert!(control_plane_url(":loadCodeAssist").ends_with(":loadCodeAssist"));
    }

    #[test]
    fn test_onboard_project_ignores_empty_hint() {
        let mut token = oauth_token();
        token
            .extra
            .insert("onboard_project".into(), Value::String(String::new()));
        assert!(test_onboard_project(&token).is_none());
    }
}
