use super::*;
use std::io::{BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::thread;

fn mock_server(responses: Vec<(u16, &'static str)>) -> String {
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
    format!("http://{addr}")
}

fn local_spec(base: String, encoding: TokenEncoding) -> ProviderSpec {
    ProviderSpec {
        name: "openai".into(),
        label: "Local".into(),
        flow: FlowKind::PasteCodePkce,
        client_id: "client".into(),
        client_secret: None,
        authorize_url: base.clone(),
        token_url: base,
        scopes: "scope".into(),
        token_encoding: encoding,
        redirect_uri: Some("https://example.com/callback".into()),
        loopback_port: None,
        loopback_host: None,
        callback_path: String::new(),
        extra_authorize: vec![],
        derived: None,
        suggested_models: vec![],
        inference: None,
    }
}

/// Load unofficial extras plugins so conformance tests still cover the
/// OAuth engine against those specs. Not used by the default binary.
///
/// Loaded once per process — never `clear_registry()`, which would race
/// parallel tests that also register specs.
fn load_extras_plugins() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../extras/auth-plugins");
        crate::plugin::load_dir(&root);
    });
}

fn oauth_providers() -> Vec<String> {
    load_extras_plugins();
    crate::spec::registered_providers()
        .into_iter()
        .filter(|n| {
            // Parallel unit tests may register throwaway specs (`demo`, …).
            matches!(
                n.as_str(),
                "anthropic" | "openai" | "google" | "github-copilot" | "xai" | "google-antigravity"
            )
        })
        .collect()
}

fn extras_spec(name: &str) -> ProviderSpec {
    load_extras_plugins();
    spec_for(name).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn derived_cred(url: impl Into<String>, scheme: &str) -> DerivedCredential {
    DerivedCredential {
        url: url.into(),
        auth_scheme: scheme.to_string(),
        headers: Vec::new(),
    }
}

struct TestUi {
    pasted: String,
}

struct LoopbackUi;

fn post_loopback_callback(url: &str, state_override: Option<&str>) {
    let url = url::Url::parse(url).unwrap();
    let state = state_override.map(str::to_string).unwrap_or_else(|| {
        url.query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned()
    });
    let redirect = url
        .query_pairs()
        .find(|(key, _)| key == "redirect_uri")
        .unwrap()
        .1
        .into_owned();
    thread::spawn(move || {
        let callback = url::Url::parse(&redirect).unwrap();
        let addr = format!(
            "{}:{}",
            callback.host_str().unwrap(),
            callback.port().unwrap()
        );
        let mut stream = std::net::TcpStream::connect(addr).unwrap();
        write!(
            stream,
            "GET {}?code=loop&state={} HTTP/1.1\r\nHost: localhost\r\n\r\n",
            callback.path(),
            state
        )
        .unwrap();
    });
}

impl LoginUi for LoopbackUi {
    fn show_sign_in(&mut self, _: &str, url: &str, _: bool) {
        post_loopback_callback(url, None);
    }
    fn note(&mut self, _: &str) {}
    fn show_device_code(&mut self, _: &str, _: &str, _: bool) {}
    async fn prompt_pasted_code(&mut self) -> Result<String> {
        Err(AuthError::FlowCancelled(
            "loopback UI cannot prompt for a pasted code".into(),
        ))
    }
}

impl LoginUi for TestUi {
    fn show_sign_in(&mut self, _: &str, _: &str, _: bool) {}
    fn note(&mut self, _: &str) {}
    fn show_device_code(&mut self, _: &str, _: &str, _: bool) {}
    async fn prompt_pasted_code(&mut self) -> Result<String> {
        Ok(self.pasted.clone())
    }
}

#[test]
fn specs_exist_for_all_advertised_providers() {
    for name in oauth_providers() {
        let spec = spec_for(&name).expect(&name);
        assert_eq!(spec.name, name);
        assert!(!spec.client_id.is_empty());
        assert!(spec.authorize_url.starts_with("https://"));
        assert!(spec.token_url.starts_with("https://"));
    }
}

#[test]
fn every_oauth_provider_suggests_at_least_one_model() {
    for name in oauth_providers() {
        assert!(
            !suggested_models(&name).is_empty(),
            "{name}: a logged-in user must see a model in the picker"
        );
    }
    assert!(suggested_models("ollama").is_empty());
}

#[test]
fn unknown_provider_is_rejected() {
    assert!(matches!(
        spec_for("mistral"),
        Err(AuthError::UnsupportedProvider(_))
    ));
    assert!(!supports_oauth("mistral"));
    load_extras_plugins();
    assert!(supports_oauth("anthropic"));
}

#[test]
fn flow_kinds_match_registered_redirects() {
    // These encode deployment facts about the public clients; if a
    // provider changes its registration this test is the tripwire.
    assert_eq!(extras_spec("openai").flow, FlowKind::LoopbackPkce);
    assert_eq!(extras_spec("openai").loopback_port, Some(1455));
    assert_eq!(extras_spec("google").flow, FlowKind::LoopbackPkce);
    assert_eq!(extras_spec("anthropic").flow, FlowKind::PasteCodePkce);
    assert_eq!(extras_spec("github-copilot").flow, FlowKind::DeviceCode);
    assert_eq!(extras_spec("xai").flow, FlowKind::LoopbackPkce);
    assert_eq!(
        extras_spec("xai").loopback_host.as_deref(),
        Some("127.0.0.1")
    );
    assert_eq!(extras_spec("xai").callback_path, "/callback");
}

#[test]
fn antigravity_requests_native_client_scopes() {
    let spec = extras_spec("google-antigravity");
    assert_eq!(spec.flow, FlowKind::LoopbackPkce);
    assert_eq!(spec.loopback_port, Some(51121));
    assert_eq!(spec.loopback_host.as_deref(), Some("127.0.0.1"));
    assert_eq!(spec.callback_path, "/oauth-callback");
    for scope in [
        "https://www.googleapis.com/auth/cloud-platform",
        "https://www.googleapis.com/auth/userinfo.email",
        "https://www.googleapis.com/auth/userinfo.profile",
        "https://www.googleapis.com/auth/cclog",
        "https://www.googleapis.com/auth/experimentsandconfigs",
    ] {
        assert!(
            spec.scopes.split_whitespace().any(|s| s == scope),
            "google-antigravity missing native scope {scope}"
        );
    }
}

#[test]
fn token_from_json_accepts_string_or_number_expires_in() {
    let t = token_from_json(&serde_json::json!({
        "access_token": "a", "refresh_token": "r", "expires_in": 3600
    }))
    .unwrap();
    assert!(t.expires_at.is_some());
    assert_eq!(t.refresh_token.as_deref(), Some("r"));
    let t = token_from_json(&serde_json::json!({
        "access_token": "a", "expires_in": "3600"
    }))
    .unwrap();
    assert!(t.expires_at.is_some());
    assert!(t.refresh_token.is_none());
}

#[test]
fn token_from_json_requires_access_token() {
    let err = token_from_json(&serde_json::json!({"token_type": "Bearer"})).unwrap_err();
    assert!(matches!(err, AuthError::TokenExchange(_)));
}

#[test]
fn token_from_json_ignores_malformed_optional_fields() {
    for json in [
        serde_json::json!({
            "access_token": "access",
            "refresh_token": "",
            "expires_in": "not-a-number"
        }),
        serde_json::json!({
            "access_token": "access",
            "refresh_token": 42,
            "expires_in": null
        }),
    ] {
        let token = token_from_json(&json).unwrap();
        assert_eq!(token.access_token, "access");
        assert!(token.refresh_token.is_none());
        assert!(token.expires_at.is_none());
    }

    let err = token_from_json(&serde_json::json!({"access_token": 42})).unwrap_err();
    assert_eq!(
        err.to_string(),
        "token exchange failed: response missing access_token"
    );
}

#[test]
fn openai_account_id_is_extracted_from_id_token() {
    // Synthetic JWT payload with the Codex account claim.
    let payload = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&serde_json::json!({
            "https://api.openai.com/auth": {"chatgpt_account_id": "acct_1"}
        }))
        .unwrap(),
    );
    let jwt = format!("header.{payload}.sig");
    assert_eq!(openai_account_id_from_jwt(&jwt).as_deref(), Some("acct_1"));
    assert!(openai_account_id_from_jwt("not-a-jwt").is_none());

    // token_from_json picks it up into `extra`.
    let t = token_from_json(&serde_json::json!({
        "access_token": "a", "id_token": jwt
    }))
    .unwrap();
    assert_eq!(
        t.extra.get("openai_account_id").and_then(Value::as_str),
        Some("acct_1")
    );
}

#[test]
fn openai_account_id_rejects_malformed_or_wrongly_typed_claims() {
    let jwt = |payload: &[u8]| format!("header.{}.sig", URL_SAFE_NO_PAD.encode(payload));

    assert!(openai_account_id_from_jwt("header.%%%.sig").is_none());
    assert!(openai_account_id_from_jwt(&jwt(b"not json")).is_none());
    assert!(
        openai_account_id_from_jwt(&jwt(&serde_json::to_vec(&serde_json::json!({
            "https://api.openai.com/auth": {"chatgpt_account_id": 7}
        }))
        .unwrap()))
        .is_none()
    );
    assert!(
        openai_account_id_from_jwt(&jwt(&serde_json::to_vec(
            &serde_json::json!({"sub": "user"})
        )
        .unwrap()))
        .is_none()
    );
}

#[test]
fn derived_usable_token_comes_from_extra() {
    let spec = extras_spec("github-copilot");
    let mut token = OAuthToken {
        access_token: "gh".to_string(),
        refresh_token: None,
        expires_at: None,
        extra: Default::default(),
    };
    assert!(usable_token(&spec, &token).is_none());
    set_derived_extra(&mut token, "cop", Utc::now() + chrono::Duration::hours(1));
    assert_eq!(usable_token(&spec, &token).as_deref(), Some("cop"));

    // Legacy "copilot_token" key (pre-rename stores) still resolves.
    let mut legacy = OAuthToken {
        access_token: "gh".to_string(),
        refresh_token: None,
        expires_at: None,
        extra: Default::default(),
    };
    legacy.extra.insert(
        "copilot_token".to_string(),
        Value::String("old".to_string()),
    );
    assert_eq!(usable_token(&spec, &legacy).as_deref(), Some("old"));
}

#[test]
fn usable_token_prefers_current_derived_key_and_checks_its_type() {
    let derived = extras_spec("github-copilot");
    let direct = extras_spec("openai");
    let mut token = OAuthToken {
        access_token: "oauth-access".to_string(),
        refresh_token: None,
        expires_at: None,
        extra: serde_json::Map::from_iter([
            (
                "derived_token".to_string(),
                Value::String("current".to_string()),
            ),
            (
                "copilot_token".to_string(),
                Value::String("legacy".to_string()),
            ),
        ]),
    };

    assert_eq!(usable_token(&derived, &token).as_deref(), Some("current"));
    assert_eq!(
        usable_token(&direct, &token).as_deref(),
        Some("oauth-access")
    );

    token
        .extra
        .insert("derived_token".to_string(), Value::Bool(true));
    assert!(usable_token(&derived, &token).is_none());
}

#[test]
fn set_derived_extra_replaces_values_in_canonical_keys() {
    let mut token = OAuthToken {
        access_token: "oauth-access".to_string(),
        refresh_token: None,
        expires_at: None,
        extra: serde_json::Map::from_iter([
            (
                "derived_token".to_string(),
                Value::String("old".to_string()),
            ),
            ("unrelated".to_string(), Value::Bool(true)),
        ]),
    };
    let expiry = DateTime::parse_from_rfc3339("2030-01-02T03:04:05Z")
        .unwrap()
        .with_timezone(&Utc);

    set_derived_extra(&mut token, "new", expiry);

    assert_eq!(token.extra["derived_token"], "new");
    assert_eq!(
        token.extra["derived_expires_at"],
        "2030-01-02T03:04:05+00:00"
    );
    assert_eq!(token.extra["unrelated"], true);
}

// ── Conformance suite ────────────────────────────────────────────────
// Adding a provider must be *only* a new spec literal in `spec_for` +
// a name in `oauth_providers()`. These tests reject malformed literals,
// drift between the moving parts, and wrong grant encodings — with no
// network involved.

#[test]
fn conformance_every_advertised_provider_validates() {
    for name in oauth_providers() {
        let spec = spec_for(&name).expect(&name);
        let issues = validate(&spec);
        assert!(issues.is_empty(), "{name}: {}", issues.join("; "));
    }
}

#[test]
fn unsupported_provider_error_mentions_plugin() {
    let msg = AuthError::UnsupportedProvider("x".to_string()).to_string();
    assert!(msg.contains("auth plugin"), "{msg}");
}

#[test]
fn conformance_authorize_url_has_required_pkce_params() {
    for name in oauth_providers() {
        let spec = spec_for(&name).expect(&name);
        if spec.flow == FlowKind::DeviceCode {
            continue; // device flow starts at the token endpoint family
        }
        let pkce = Pkce {
            verifier: "v".to_string(),
            challenge: "c".to_string(),
            state: "s".to_string(),
        };
        let redirect = spec
            .redirect_uri
            .clone()
            .unwrap_or_else(|| format!("http://localhost:9999{}", spec.callback_path));
        let url = flow::authorize_url(
            &spec.authorize_url,
            &spec.client_id,
            &redirect,
            &spec.scopes,
            &pkce,
            &spec.extra_authorize,
        );
        for param in [
            "response_type=code",
            "client_id=",
            "redirect_uri=",
            "scope=",
            "state=s",
            "code_challenge=c",
            "code_challenge_method=S256",
        ] {
            assert!(
                url.contains(param),
                "{name}: authorize URL missing `{param}`"
            );
        }
        // The verifier must never appear in a URL.
        assert!(
            !url.contains("code_verifier"),
            "{name}: verifier leaked into URL"
        );
    }
}

#[test]
fn conformance_grant_bodies_match_declared_encoding() {
    let pkce = Pkce {
        verifier: "ver".to_string(),
        challenge: "ch".to_string(),
        state: "st".to_string(),
    };
    for name in oauth_providers() {
        let spec = spec_for(&name).expect(&name);
        if spec.flow == FlowKind::DeviceCode {
            continue;
        }
        match (
            spec.token_encoding,
            code_exchange_body(&spec, "code1", "http://localhost/cb", &pkce),
            refresh_body(&spec, "ref1"),
        ) {
            (TokenEncoding::Json, GrantBody::Json(ex), GrantBody::Json(re)) => {
                assert_eq!(ex["grant_type"], "authorization_code");
                assert_eq!(ex["state"], "st");
                assert_eq!(ex["code_verifier"], "ver");
                assert_eq!(re["grant_type"], "refresh_token");
            }
            (TokenEncoding::Form, GrantBody::Form(ex), GrantBody::Form(re)) => {
                fn form_get<'a>(f: &'a [(&'static str, String)], k: &str) -> Option<&'a str> {
                    f.iter().find(|(fk, _)| fk == &k).map(|(_, v)| v.as_str())
                }
                assert_eq!(form_get(&ex, "grant_type"), Some("authorization_code"));
                assert_eq!(form_get(&ex, "code_verifier"), Some("ver"));
                assert_eq!(form_get(&re, "refresh_token"), Some("ref1"));
                // client_secret present iff the spec declares one.
                assert_eq!(
                    form_get(&ex, "client_secret").is_some(),
                    spec.client_secret.is_some(),
                    "{name}: client_secret / form mismatch"
                );
            }
            _ => unreachable!("grant constructors are exhaustive over token_encoding: {name}"),
        }
    }
}

#[test]
fn conformance_validate_catches_broken_specs() {
    // Guard the guard: a deliberately broken spec must produce issues.
    let mut broken = extras_spec("google");
    broken.client_id = String::new();
    broken.flow = FlowKind::PasteCodePkce; // …but redirect_uri stays None
    broken.authorize_url = "http://insecure.example.com".into();
    let issues = validate(&broken);
    assert!(issues.len() >= 3, "expected >=3 issues, got: {issues:?}");
}

#[test]
fn validate_reports_each_flow_specific_error_branch() {
    let mut loopback = extras_spec("openai");
    loopback.callback_path = "callback".into();
    loopback.redirect_uri = Some("https://example.com/callback".into());
    loopback.loopback_host = Some("example.com".into());
    let issues = validate(&loopback).join("\n");
    assert!(issues.contains("callback_path starting with '/'"));
    assert!(issues.contains("set redirect_uri = None"));
    assert!(issues.contains("loopback_host must be"));

    let mut paste = extras_spec("anthropic");
    paste.redirect_uri = Some("http://insecure.example/callback".into());
    paste.loopback_host = Some("localhost".into());
    let issues = validate(&paste).join("\n");
    assert!(issues.contains("registered https redirect_uri"));
    assert!(issues.contains("loopback_host is only for LoopbackPkce"));

    let mut device = extras_spec("github-copilot");
    device.callback_path = "/callback".into();
    device.redirect_uri = Some("https://example.com/callback".into());
    let issues = validate(&device).join("\n");
    assert!(issues.contains("device flow has no redirect"));
}

#[test]
fn validate_reports_metadata_extra_and_derived_errors() {
    let mut spec = extras_spec("github-copilot");
    spec.label = String::new();
    spec.client_id = "client id".into();
    spec.token_url = "relative".into();
    spec.scopes = "  ".into();
    spec.client_secret = Some(String::new());
    spec.extra_authorize = vec![
        (String::new(), "value".into()),
        ("same".into(), "one".into()),
        ("same".into(), "two".into()),
    ];
    spec.derived = Some(DerivedCredential {
        url: "http://insecure.example/token".into(),
        auth_scheme: "Basic".into(),
        headers: vec![
            (String::new(), "value".into()),
            ("key".into(), String::new()),
        ],
    });

    let issues = validate(&spec).join("\n");
    for expected in [
        "label is empty",
        "client_id is empty or contains whitespace",
        "token_url must be an absolute https URL",
        "scopes are empty",
        "client_secret is Some(\"\")",
        "extra_authorize[0] has an empty key or value",
        "extra_authorize has duplicate key `same`",
        "derived.url must be an absolute https URL",
        "derived.auth_scheme must be",
        "derived.headers has an empty key or value",
    ] {
        assert!(
            issues.contains(expected),
            "missing `{expected}` in:\n{issues}"
        );
    }
}

#[tokio::test]
async fn access_token_resolves_fresh_direct_credentials() {
    load_extras_plugins();
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path());
    store
        .set(
            "openai",
            ProviderAuth {
                method: "imported".to_string(),
                token: OAuthToken {
                    access_token: "stored-access".to_string(),
                    refresh_token: None,
                    expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
                    extra: Default::default(),
                },
            },
        )
        .unwrap();

    assert_eq!(
        access_token("openai", dir.path()).await.as_deref(),
        Some("stored-access")
    );
    assert!(access_token("unknown", dir.path()).await.is_none());
    assert!(access_token("google", dir.path()).await.is_none());
}

#[tokio::test]
async fn force_refresh_without_login_is_none() {
    load_extras_plugins();
    let dir = tempfile::tempdir().unwrap();
    assert!(force_refresh("anthropic", dir.path()).await.is_none());
}

#[tokio::test]
async fn force_refresh_without_renewal_path_keeps_credential() {
    // A stored credential with no refresh token (and no derived
    // exchange) cannot be renewed: force_refresh must return None and
    // must NOT delete the stored credential.
    load_extras_plugins();
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path());
    store
        .set(
            "anthropic",
            ProviderAuth {
                method: "oauth".to_string(),
                token: OAuthToken {
                    access_token: "acc".to_string(),
                    refresh_token: None,
                    expires_at: None,
                    extra: Default::default(),
                },
            },
        )
        .unwrap();
    assert!(force_refresh("anthropic", dir.path()).await.is_none());
    assert!(store.get("anthropic").unwrap().is_some());
}

fn device_spec(base: String) -> ProviderSpec {
    let mut spec = local_spec(base, TokenEncoding::Form);
    spec.flow = FlowKind::DeviceCode;
    spec.redirect_uri = None;
    spec
}

fn loopback_spec(base: String) -> ProviderSpec {
    let mut spec = local_spec(base, TokenEncoding::Form);
    spec.flow = FlowKind::LoopbackPkce;
    spec.redirect_uri = None;
    spec.callback_path = "/callback".into();
    spec
}

fn expired_token(refresh: Option<&str>) -> OAuthToken {
    OAuthToken {
        access_token: "old".into(),
        refresh_token: refresh.map(str::to_string),
        expires_at: Some(Utc::now()),
        extra: Default::default(),
    }
}

#[tokio::test]
async fn refresh_grant_returns_access_token() {
    let spec = local_spec(
        mock_server(vec![(
            200,
            r#"{"access_token":"a","refresh_token":"r","expires_in":3600}"#,
        )]),
        TokenEncoding::Json,
    );
    assert_eq!(refresh_grant(&spec, "r").await.unwrap().access_token, "a");
}

#[tokio::test]
async fn refresh_grant_surfaces_provider_error_bodies() {
    for (body, expected) in [
        (r#"{"error_description":"bad grant"}"#, "bad grant"),
        (r#"{"error":{"message":"nested"}}"#, "nested"),
        (r#"{"error":"plain"}"#, "plain"),
        (r#"{}"#, "unknown error"),
    ] {
        let spec = local_spec(mock_server(vec![(400, body)]), TokenEncoding::Form);
        let err = refresh_grant(&spec, "r").await.unwrap_err();
        assert!(err.to_string().contains(expected), "{err}");
    }
}

#[tokio::test]
async fn exchange_derived_token_returns_token() {
    let derived = DerivedCredential {
        url: mock_server(vec![(
            200,
            r#"{"token":"derived","expires_at":1893456000}"#,
        )]),
        auth_scheme: "Bearer".into(),
        headers: vec![("X-Test".into(), "yes".into())],
    };
    assert_eq!(
        exchange_derived_token(&derived, "oauth").await.unwrap().0,
        "derived"
    );
}

#[tokio::test]
async fn exchange_derived_token_reports_http_and_shape_errors() {
    for (status, body, expected) in [
        (401, r#"{"message":"denied"}"#, "denied"),
        (200, r#"{"expires_at":1893456000}"#, "missing token"),
        (200, r#"{"token":"x"}"#, "missing expires_at"),
    ] {
        let derived = derived_cred(mock_server(vec![(status, body)]), "Bearer");
        let err = exchange_derived_token(&derived, "oauth").await.unwrap_err();
        assert!(err.to_string().contains(expected), "{err}");
    }
}

#[tokio::test]
async fn paste_code_login_rejects_empty_paste() {
    let spec = local_spec(mock_server(vec![(200, r#"{}"#)]), TokenEncoding::Form);
    let err = paste_code_login(
        &spec,
        false,
        &mut TestUi {
            pasted: String::new(),
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("no code"), "{err}");
}

#[tokio::test]
async fn paste_code_login_rejects_state_mismatch() {
    let spec = local_spec(mock_server(vec![(200, r#"{}"#)]), TokenEncoding::Form);
    let err = paste_code_login(
        &spec,
        false,
        &mut TestUi {
            pasted: "code#wrong-state".into(),
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("state mismatch"), "{err}");
}

#[tokio::test]
async fn paste_code_login_exchanges_code() {
    let spec = local_spec(
        mock_server(vec![(200, r#"{"access_token":"signed-in"}"#)]),
        TokenEncoding::Form,
    );
    let token = paste_code_login(
        &spec,
        false,
        &mut TestUi {
            pasted: "code".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(token.access_token, "signed-in");
}

#[tokio::test]
async fn paste_code_login_requires_redirect_uri() {
    let mut spec = local_spec(mock_server(vec![(200, r#"{}"#)]), TokenEncoding::Form);
    spec.redirect_uri = None;
    let err = paste_code_login(
        &spec,
        false,
        &mut TestUi {
            pasted: "code".into(),
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("redirect_uri"), "{err}");
}

#[tokio::test]
async fn login_rejects_unknown_provider() {
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path());
    assert!(login("unknown", &store, false).await.is_err());
    assert!(
        login_with_ui(
            "unknown",
            &store,
            false,
            &mut TestUi {
                pasted: String::new()
            }
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn login_with_spec_persists_paste_flow() {
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path());
    let spec = local_spec(
        mock_server(vec![(200, r#"{"access_token":"paste"}"#)]),
        TokenEncoding::Form,
    );
    let auth = login_with_spec(
        &spec,
        &store,
        false,
        &mut TestUi {
            pasted: "code".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(auth.token.access_token, "paste");
    assert_eq!(store.get("openai").unwrap().unwrap().method, "oauth");
}

#[tokio::test]
async fn login_with_spec_persists_loopback_flow() {
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path());
    let spec = loopback_spec(mock_server(vec![(200, r#"{"access_token":"loopback"}"#)]));
    let auth = login_with_spec(&spec, &store, false, &mut LoopbackUi)
        .await
        .unwrap();
    assert_eq!(auth.token.access_token, "loopback");
}

#[tokio::test]
async fn loopback_login_reports_busy_fixed_port() {
    let (occupied, port) = flow::bind_loopback().unwrap();
    let mut spec = loopback_spec("http://127.0.0.1:1".into());
    spec.loopback_port = Some(port);
    let err = loopback_login(&spec, false, &mut LoopbackUi)
        .await
        .unwrap_err();
    drop(occupied);
    assert!(
        err.to_string().contains("port") && err.to_string().contains("busy"),
        "{err}"
    );
}

#[tokio::test]
async fn loopback_login_binds_an_explicit_port() {
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path());
    let mut spec = loopback_spec(mock_server(vec![(200, r#"{"access_token":"p0"}"#)]));
    spec.loopback_port = Some(0);
    let auth = login_with_spec(&spec, &store, false, &mut LoopbackUi)
        .await
        .unwrap();
    assert_eq!(auth.token.access_token, "p0");
}

struct MismatchLoopbackUi;

impl LoginUi for MismatchLoopbackUi {
    fn show_sign_in(&mut self, _: &str, url: &str, _: bool) {
        post_loopback_callback(url, Some("nope"));
    }
    fn note(&mut self, _: &str) {}
    fn show_device_code(&mut self, _: &str, _: &str, _: bool) {}
    async fn prompt_pasted_code(&mut self) -> Result<String> {
        Err(AuthError::FlowCancelled("no paste".into()))
    }
}

#[tokio::test]
async fn loopback_login_propagates_callback_errors() {
    let mut spec = loopback_spec("http://127.0.0.1:1".into());
    spec.loopback_port = Some(0);
    let err = loopback_login(&spec, false, &mut MismatchLoopbackUi)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("state mismatch"), "{err}");
}

#[tokio::test]
async fn device_flow_exchanges_oauth_and_derived_tokens() {
    let base = mock_server(vec![
        (
            200,
            r#"{"device_code":"dev","user_code":"USER","verification_uri":"https://example.com/device","interval":0,"expires_in":30}"#,
        ),
        (200, r#"{"access_token":"github"}"#),
        (200, r#"{"token":"copilot","expires_at":1893456000}"#),
    ]);
    let mut spec = device_spec(base.clone());
    spec.derived = Some(derived_cred(base, "token"));
    let mut ui = TestUi {
        pasted: String::new(),
    };
    let dir = tempfile::tempdir().unwrap();
    let token = login_with_spec(&spec, &TokenStore::new(dir.path()), false, &mut ui)
        .await
        .unwrap()
        .token;
    assert_eq!(token.access_token, "github");
    assert_eq!(token.extra["derived_token"], "copilot");
}

#[tokio::test]
async fn device_login_maps_protocol_errors() {
    for (start, poll, expected) in [
        (r#"{}"#, None, "missing device_code"),
        (
            r#"{"device_code":"dev","interval":1,"expires_in":30}"#,
            Some(r#"{"error":"expired_token"}"#),
            "device code expired",
        ),
        (
            r#"{"device_code":"dev","interval":1,"expires_in":30}"#,
            Some(r#"{"error":"denied","error_description":"user said no"}"#),
            "user said no",
        ),
        (
            r#"{"device_code":"dev","interval":1,"expires_in":30}"#,
            Some(r#"{}"#),
            "missing access_token",
        ),
    ] {
        let mut responses = vec![(200, start)];
        if let Some(poll) = poll {
            responses.push((200, poll));
        }
        let spec = device_spec(mock_server(responses));
        let err = device_login(
            &spec,
            false,
            &mut TestUi {
                pasted: String::new(),
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains(expected), "{err}");
    }
}

#[tokio::test]
async fn device_login_expires_before_authorization() {
    let spec = device_spec(mock_server(vec![(
        200,
        r#"{"device_code":"dev","interval":1,"expires_in":0}"#,
    )]));
    let err = device_login(
        &spec,
        false,
        &mut TestUi {
            pasted: String::new(),
        },
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("expired before authorization"),
        "{err}"
    );
}

#[tokio::test]
async fn device_login_returns_oauth_token() {
    let spec = device_spec(mock_server(vec![
        (200, r#"{"device_code":"dev","interval":1,"expires_in":30}"#),
        (200, r#"{"access_token":"github"}"#),
    ]));
    assert_eq!(
        device_login(
            &spec,
            false,
            &mut TestUi {
                pasted: String::new()
            }
        )
        .await
        .unwrap()
        .access_token,
        "github"
    );
}

#[tokio::test]
async fn ensure_fresh_refreshes_expired_token() {
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path());
    let spec = local_spec(
        mock_server(vec![(200, r#"{"access_token":"refreshed"}"#)]),
        TokenEncoding::Form,
    );
    assert_eq!(
        ensure_fresh(&spec, &store, "imported", expired_token(Some("refresh")))
            .await
            .unwrap()
            .access_token,
        "refreshed"
    );
}

#[tokio::test]
async fn force_fresh_refreshes_expired_token() {
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path());
    let spec = local_spec(
        mock_server(vec![(200, r#"{"access_token":"forced"}"#)]),
        TokenEncoding::Form,
    );
    assert_eq!(
        force_fresh(&spec, &store, "imported", expired_token(Some("refresh")))
            .await
            .unwrap()
            .access_token,
        "forced"
    );
}

#[tokio::test]
async fn ensure_fresh_derived_exchanges_when_stale() {
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path());
    let base = mock_server(vec![(
        200,
        r#"{"token":"new-derived","expires_at":1893456000}"#,
    )]);
    let mut spec = local_spec(base.clone(), TokenEncoding::Form);
    spec.derived = Some(derived_cred(base, "token"));
    let oauth = OAuthToken {
        access_token: "oauth".into(),
        refresh_token: None,
        expires_at: None,
        extra: Default::default(),
    };
    assert_eq!(
        ensure_fresh(&spec, &store, "imported", oauth)
            .await
            .unwrap()
            .extra["derived_token"],
        "new-derived"
    );
}

#[tokio::test]
async fn force_fresh_reexchanges_derived_token() {
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path());
    let base = mock_server(vec![(
        200,
        r#"{"token":"forced-derived","expires_at":1893456000}"#,
    )]);
    let mut spec = local_spec(base.clone(), TokenEncoding::Form);
    spec.derived = Some(derived_cred(base, "token"));
    let oauth = OAuthToken {
        access_token: "oauth".into(),
        refresh_token: None,
        expires_at: None,
        extra: Default::default(),
    };
    assert_eq!(
        force_fresh(&spec, &store, "imported", oauth)
            .await
            .unwrap()
            .extra["derived_token"],
        "forced-derived"
    );
}

#[tokio::test]
async fn reexchange_derived_requires_derived_spec() {
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path());
    let spec = local_spec("http://127.0.0.1:1".into(), TokenEncoding::Form);
    let oauth = OAuthToken {
        access_token: "oauth".into(),
        refresh_token: None,
        expires_at: None,
        extra: Default::default(),
    };
    assert!(
        reexchange_derived(&spec, &store, "oauth", oauth)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn ensure_fresh_derived_keeps_unexpired_token() {
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path());
    let mut spec = local_spec("http://127.0.0.1:1".into(), TokenEncoding::Form);
    spec.derived = Some(derived_cred("http://127.0.0.1:1", "token"));
    let mut fresh = OAuthToken {
        access_token: "oauth".into(),
        refresh_token: None,
        expires_at: None,
        extra: Default::default(),
    };
    set_derived_extra(
        &mut fresh,
        "still-fresh",
        Utc::now() + chrono::Duration::hours(1),
    );
    assert_eq!(
        ensure_fresh_derived(&spec, &store, "imported", fresh)
            .await
            .unwrap()
            .extra["derived_token"],
        "still-fresh"
    );
}

#[tokio::test]
async fn ensure_fresh_maps_provider_refresh_errors() {
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path());
    let spec = local_spec(
        mock_server(vec![(400, r#"{"error":"rejected"}"#)]),
        TokenEncoding::Form,
    );
    assert!(matches!(
        ensure_fresh(&spec, &store, "oauth", expired_token(Some("refresh"))).await,
        Err(AuthError::Refresh(_, _))
    ));
}

#[tokio::test]
async fn force_fresh_maps_provider_refresh_errors() {
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path());
    let spec = local_spec(
        mock_server(vec![(400, r#"{"error":"rejected"}"#)]),
        TokenEncoding::Form,
    );
    assert!(matches!(
        force_fresh(&spec, &store, "oauth", expired_token(Some("refresh"))).await,
        Err(AuthError::Refresh(_, _))
    ));
}

#[tokio::test]
async fn ensure_fresh_without_refresh_token_is_not_logged_in() {
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path());
    let spec = local_spec("http://127.0.0.1:1".into(), TokenEncoding::Form);
    assert!(matches!(
        ensure_fresh(&spec, &store, "oauth", expired_token(None)).await,
        Err(AuthError::NotLoggedIn(_))
    ));
}

#[tokio::test]
async fn force_refresh_with_spec_renews_stored_credential() {
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path());
    let spec = local_spec(
        mock_server(vec![(200, r#"{"access_token":"wrapper-refreshed"}"#)]),
        TokenEncoding::Form,
    );
    store
        .set(
            &spec.name,
            ProviderAuth {
                method: "imported".into(),
                token: OAuthToken {
                    access_token: "old".into(),
                    refresh_token: Some("refresh".into()),
                    expires_at: None,
                    extra: Default::default(),
                },
            },
        )
        .unwrap();
    assert_eq!(
        force_refresh_with_spec(&spec, dir.path()).await.as_deref(),
        Some("wrapper-refreshed")
    );
}

#[tokio::test]
async fn login_with_ui_dispatches_a_known_provider() {
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path());
    let mut ui = TestUi {
        pasted: String::new(),
    };
    let error = login_with_ui("anthropic", &store, false, &mut ui)
        .await
        .unwrap_err();
    assert!(matches!(error, AuthError::FlowCancelled(_)));
}

#[tokio::test]
async fn cli_ui_prints_and_open_browser_fails_on_bad_url() {
    let mut ui = CliLoginUi;
    ui.show_sign_in("Provider", "https://example.com", true);
    ui.show_sign_in("Provider", "https://example.com", false);
    ui.note("note");
    ui.show_device_code("CODE", "https://example.com/device", true);
    ui.show_device_code("CODE", "https://example.com/device", false);
    assert_eq!(
        read_pasted_code(&mut std::io::Cursor::new("code#state\n")).unwrap(),
        "code#state"
    );
    let err = read_pasted_code(&mut FailRead).unwrap_err();
    assert!(matches!(err, AuthError::Io(_)));
    assert!(!try_open_browser("invalid URL\0"));
    assert!(!maybe_open_browser(false, "https://example.com"));
    assert!(!maybe_open_browser(true, "invalid URL\0"));
    assert_eq!(
        join_blocking_paste(Ok(Ok(" pasted ".into()))).unwrap(),
        " pasted "
    );
    assert!(matches!(
        join_blocking_paste(Ok(Err(AuthError::Io(std::io::Error::other("e"))))),
        Err(AuthError::Io(_))
    ));
    // Bound so a TTY cannot hang the suite; EOF (CI) returns immediately.
    let _ = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        ui.prompt_pasted_code(),
    )
    .await;
}

#[tokio::test]
async fn cli_login_ui_instantiates_each_flow_kind() {
    let spec = device_spec(mock_server(vec![
        (200, r#"{"device_code":"dev","interval":1,"expires_in":30}"#),
        (200, r#"{"access_token":"github"}"#),
    ]));
    assert_eq!(
        device_login(&spec, false, &mut CliLoginUi)
            .await
            .unwrap()
            .access_token,
        "github"
    );

    let spec = local_spec(mock_server(vec![(200, r#"{}"#)]), TokenEncoding::Form);
    let err = paste_code_login(&spec, false, &mut CliLoginUi)
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::FlowCancelled(_)), "{err}");

    let spec = loopback_spec("http://127.0.0.1:1".into());
    let err = loopback_login(&spec, false, &mut CliLoginUi)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("timed out"), "{err}");
}

#[tokio::test]
async fn join_helpers_map_cancelled_tasks() {
    let paste = tokio::spawn(std::future::pending::<Result<String>>());
    paste.abort();
    let err = join_blocking_paste(paste.await).unwrap_err();
    assert!(
        matches!(err, AuthError::FlowCancelled(ref msg) if msg.contains("stdin task failed")),
        "{err}"
    );

    let callback = tokio::spawn(std::future::pending::<Result<flow::CallbackResult>>());
    callback.abort();
    let err = join_blocking_callback(callback.await).unwrap_err();
    assert!(
        matches!(err, AuthError::FlowCancelled(ref msg) if msg.contains("callback task failed")),
        "{err}"
    );
}

struct FailRead;

impl std::io::Read for FailRead {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("fail"))
    }
}

impl std::io::BufRead for FailRead {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        Err(std::io::Error::other("fail"))
    }
    fn consume(&mut self, _: usize) {}
}

#[tokio::test]
async fn device_login_waits_through_pending_and_slow_down() {
    let spec = device_spec(mock_server(vec![
        (200, r#"{"device_code":"dev","interval":1,"expires_in":30}"#),
        (200, r#"{"error":"authorization_pending"}"#),
        (200, r#"{"error":"slow_down"}"#),
        (200, r#"{"error":"denied"}"#),
    ]));
    let err = device_login(
        &spec,
        false,
        &mut TestUi {
            pasted: String::new(),
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("denied"), "{err}");
}

#[tokio::test]
async fn device_login_defaults_missing_poll_fields_and_http_errors() {
    let spec = device_spec(mock_server(vec![
        (200, r#"{"device_code":"dev","interval":1,"expires_in":30}"#),
        (200, r#"{"error":"authorization_pending"}"#),
        (200, r#"{"access_token":"github"}"#),
    ]));
    assert_eq!(
        device_login(
            &spec,
            false,
            &mut TestUi {
                pasted: String::new()
            }
        )
        .await
        .unwrap()
        .access_token,
        "github"
    );

    let spec = device_spec(mock_server(vec![(401, r#"{"message":"nope"}"#)]));
    let err = device_login(
        &spec,
        false,
        &mut TestUi {
            pasted: String::new(),
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AuthError::Http(_)), "{err}");
}

#[tokio::test]
async fn force_fresh_without_refresh_token_is_not_logged_in() {
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path());
    let spec = local_spec("http://127.0.0.1:1".into(), TokenEncoding::Form);
    assert!(matches!(
        force_fresh(&spec, &store, "oauth", expired_token(None)).await,
        Err(AuthError::NotLoggedIn(_))
    ));
}

#[test]
fn error_messages_cover_every_variant() {
    let io = AuthError::from(std::io::Error::other("disk"));
    let json = AuthError::from(serde_json::from_str::<i32>("nope").unwrap_err());
    for err in [
        AuthError::UnsupportedProvider("x".into()),
        AuthError::NotLoggedIn("openai".into()),
        AuthError::InsecureStorePermissions("/tmp/auth.json".into()),
        AuthError::FlowCancelled("timeout".into()),
        AuthError::Provider("denied".into()),
        AuthError::TokenExchange("bad".into()),
        AuthError::Refresh("openai".into(), "revoked".into()),
        AuthError::ConsentRequired("/tmp/cred.json".into()),
        AuthError::SymlinkRejected("/tmp/link".into()),
        AuthError::BrowserUnavailable("https://example.com".into()),
        io,
        json,
    ] {
        assert!(!err.to_string().is_empty(), "{err:?}");
    }
}
