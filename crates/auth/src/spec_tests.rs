use super::*;

#[test]
fn unknown_name_is_unsupported() {
    assert!(!supports_oauth("definitely-missing-oauth-provider"));
    assert!(spec_for("definitely-missing-oauth-provider").is_err());
    assert!(spec_get("definitely-missing-oauth-provider").is_none());
    assert!(suggested_models("definitely-missing-oauth-provider").is_empty());
    assert!(inference_identity("definitely-missing-oauth-provider").is_none());
}

#[test]
fn register_then_lookup() {
    register_spec(ProviderSpec {
        name: "spec-register-demo".into(),
        label: "Demo".into(),
        flow: FlowKind::DeviceCode,
        client_id: "cid".into(),
        client_secret: None,
        authorize_url: "https://example.com/auth".into(),
        token_url: "https://example.com/token".into(),
        scopes: "read".into(),
        token_encoding: TokenEncoding::Form,
        redirect_uri: None,
        loopback_port: None,
        loopback_host: None,
        callback_path: String::new(),
        extra_authorize: vec![],
        derived: None,
        suggested_models: vec!["m1".into()],
        inference: None,
    });
    assert!(supports_oauth("spec-register-demo"));
    assert!(
        registered_providers()
            .iter()
            .any(|n| n == "spec-register-demo")
    );
    assert_eq!(
        suggested_models("spec-register-demo"),
        vec!["m1".to_string()]
    );
    // Drop only this spec so parallel tests keep extras plugins.
    lock_registry().remove("spec-register-demo");
}

#[test]
fn clear_registry_is_restored_from_saved_specs() {
    let saved: Vec<ProviderSpec> = lock_registry().values().cloned().collect();
    register_spec(ProviderSpec {
        name: "spec-clear-demo".into(),
        label: "Demo".into(),
        flow: FlowKind::DeviceCode,
        client_id: "cid".into(),
        client_secret: None,
        authorize_url: "https://example.com/auth".into(),
        token_url: "https://example.com/token".into(),
        scopes: "read".into(),
        token_encoding: TokenEncoding::Form,
        redirect_uri: None,
        loopback_port: None,
        loopback_host: None,
        callback_path: String::new(),
        extra_authorize: vec![],
        derived: None,
        suggested_models: vec![],
        inference: None,
    });
    clear_registry();
    assert!(!supports_oauth("spec-clear-demo"));
    for spec in saved {
        register_spec(spec);
    }
}

#[test]
fn lock_registry_recovers_from_poison() {
    let _saved: Vec<ProviderSpec> = lock_registry().values().cloned().collect();
    let handle = std::thread::spawn(|| {
        let _guard = lock_registry();
        panic!("poison the auth spec registry");
    });
    let _ = handle.join();
    let _guard = lock_registry();
    drop(_guard);
    for spec in _saved {
        register_spec(spec);
    }
}
