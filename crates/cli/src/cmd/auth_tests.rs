use super::*;

#[test]
fn expiry_label_none_is_none() {
    let tok = whycodes_auth::OAuthToken {
        access_token: "t".into(),
        refresh_token: None,
        expires_at: None,
        extra: Default::default(),
    };
    let auth = whycodes_auth::ProviderAuth {
        method: "oauth".into(),
        token: tok,
    };
    assert!(!auth_expiry_label(&auth).is_empty());
}

#[test]
fn expiry_label_derived_and_expired_and_future() {
    let mut extra = serde_json::Map::new();
    extra.insert(
        "derived_expires_at".into(),
        serde_json::Value::String("2099-01-01".into()),
    );
    let derived = whycodes_auth::ProviderAuth {
        method: "oauth".into(),
        token: whycodes_auth::OAuthToken {
            access_token: "t".into(),
            refresh_token: None,
            expires_at: None,
            extra,
        },
    };
    assert!(auth_expiry_label(&derived).contains("derived API token"));

    let mut extra = serde_json::Map::new();
    extra.insert(
        "copilot_expires_at".into(),
        serde_json::Value::String("2099-01-01".into()),
    );
    let legacy = whycodes_auth::ProviderAuth {
        method: "oauth".into(),
        token: whycodes_auth::OAuthToken {
            access_token: "t".into(),
            refresh_token: None,
            expires_at: None,
            extra,
        },
    };
    assert!(auth_expiry_label(&legacy).contains("derived API token"));

    let expired = whycodes_auth::ProviderAuth {
        method: "oauth".into(),
        token: whycodes_auth::OAuthToken {
            access_token: "t".into(),
            refresh_token: None,
            expires_at: Some(chrono::Utc::now() - chrono::Duration::hours(2)),
            extra: Default::default(),
        },
    };
    assert!(auth_expiry_label(&expired).contains("expired"));

    let future = whycodes_auth::ProviderAuth {
        method: "oauth".into(),
        token: whycodes_auth::OAuthToken {
            access_token: "t".into(),
            refresh_token: None,
            expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(2)),
            extra: Default::default(),
        },
    };
    assert!(auth_expiry_label(&future).contains("expires"));
}

#[test]
fn auth_printer_and_prompt_helpers() {
    assert!(logged_in_line("acme", "/tmp/auth.json").contains("acme"));
    assert!(logout_removed_line("acme").contains("acme"));
    assert_eq!(
        logout_missing_line("acme"),
        "No stored credentials for `acme`."
    );
    assert!(auth_status_empty_line("anthropic").contains("anthropic"));
    assert!(import_none_found_line("Claude Code").contains("Claude Code"));
    assert!(import_prompt_yes("y"));
    assert!(import_prompt_yes("YES"));
    assert!(!import_prompt_yes("n"));
    assert!(!import_prompt_yes(""));
    assert!(skipped_consent_line("/tmp/consent").contains("/tmp/consent"));
    assert!(
        import_state_label(whycodes_auth::discover::SourceState::Denied)
            .to_string()
            .contains("denied")
    );
    assert!(
        import_state_label(whycodes_auth::discover::SourceState::New)
            .to_string()
            .contains("new")
    );
    assert!(
        import_state_label(whycodes_auth::discover::SourceState::Approved)
            .to_string()
            .contains("approved")
    );
    assert!(
        import_state_label(whycodes_auth::discover::SourceState::Symlink)
            .to_string()
            .contains("symlink")
    );
    assert!(imported_count_line(2).contains("2"));
    let dir = auth_data_dir();
    assert!(!dir.as_os_str().is_empty());
}
