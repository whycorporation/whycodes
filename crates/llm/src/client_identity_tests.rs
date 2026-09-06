use super::*;

#[test]
fn user_agent_starts_with_whycodes() {
    assert!(
        USER_AGENT.starts_with("whycodes/"),
        "USER_AGENT={USER_AGENT}"
    );
    assert!(!USER_AGENT.ends_with('/'));
    assert!(USER_AGENT.len() > "whycodes/".len());
}

#[test]
fn identity_constants() {
    assert_eq!(X_TITLE, "whycodes");
    assert!(HTTP_REFERER.contains("why.codes"));
}

#[test]
fn http_client_builds() {
    let _ = http_client();
}

#[test]
fn http_client_is_shared() {
    // Process-wide client: same static reference on every call.
    assert!(std::ptr::eq(shared_client(), shared_client()));
    // Clones are cheap and keep the pool warm.
    let _a = http_client();
    let _b = http_client();
}

#[test]
fn plugin_identity_falls_back_to_whycodes() {
    let _ = with_plugin_identity(http_client().get("https://example.invalid/"), "no-such");
}

#[test]
fn connect_timeout_is_finite() {
    // Guard against regressions that drop connect_timeout and re-inflate
    // "Worked for Xs" on dead VPN/Tailscale hops.
    assert!(CONNECT_TIMEOUT.as_secs() >= 1);
    assert!(CONNECT_TIMEOUT.as_secs() <= 10);
}

#[test]
fn plugin_identity_applies_ua_and_skips_user_agent_header() {
    use std::collections::HashMap;
    use whycodes_auth::{FlowKind, InferenceIdentity, ProviderSpec, TokenEncoding};

    let name = format!("plugin-id-{}", std::process::id());
    whycodes_auth::register_spec(ProviderSpec {
        name: name.clone(),
        label: "Plugin".into(),
        flow: FlowKind::DeviceCode,
        client_id: "cid".into(),
        client_secret: None,
        authorize_url: "https://example.invalid/a".into(),
        token_url: "https://example.invalid/t".into(),
        scopes: "read".into(),
        token_encoding: TokenEncoding::Form,
        redirect_uri: None,
        loopback_port: None,
        loopback_host: None,
        callback_path: String::new(),
        extra_authorize: vec![],
        derived: None,
        suggested_models: vec![],
        inference: Some(InferenceIdentity {
            user_agent: Some("plugin-ua".into()),
            headers: HashMap::from([
                ("originator".into(), "whycodes".into()),
                ("User-Agent".into(), "skip-me".into()),
            ]),
        }),
    });
    let _ = with_plugin_identity(http_client().get("https://example.invalid/"), &name);
    let _ = post_for_provider("https://example.invalid/", &name);

    whycodes_auth::register_spec(ProviderSpec {
        name: format!("{name}-headers-only"),
        label: "Plugin".into(),
        flow: FlowKind::DeviceCode,
        client_id: "cid".into(),
        client_secret: None,
        authorize_url: "https://example.invalid/a".into(),
        token_url: "https://example.invalid/t".into(),
        scopes: "read".into(),
        token_encoding: TokenEncoding::Form,
        redirect_uri: None,
        loopback_port: None,
        loopback_host: None,
        callback_path: String::new(),
        extra_authorize: vec![],
        derived: None,
        suggested_models: vec![],
        inference: Some(InferenceIdentity {
            user_agent: Some(String::new()),
            headers: HashMap::from([("originator".into(), "whycodes".into())]),
        }),
    });
    let _ = with_plugin_identity(
        http_client().get("https://example.invalid/"),
        &format!("{name}-headers-only"),
    );
    let _ = post("https://example.invalid/");
}
