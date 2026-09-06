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
    let err =
        surface_free_tier_ineligibility(&ineligible).expect_err("should surface Google's reason");
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
    lock_poison(&LOCK)
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
        *lock_poison(test_base()) = None;
    }
}

fn set_test_base(base: String) -> TestBaseGuard {
    *lock_poison(test_base()) = Some(base);
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

#[test]
fn env_restore_puts_back_existing_project() {
    let _lock = env_lock();
    unsafe { std::env::set_var("GOOGLE_CLOUD_PROJECT", "keep-me") };
    {
        let _restore = EnvRestore::capture();
        unsafe { std::env::remove_var("GOOGLE_CLOUD_PROJECT") };
    }
    assert_eq!(
        std::env::var("GOOGLE_CLOUD_PROJECT").as_deref(),
        Ok("keep-me")
    );
    unsafe { std::env::remove_var("GOOGLE_CLOUD_PROJECT") };
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
    *lock_poison(test_base()) = None;
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
    *lock_poison(test_base()) = None;
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
