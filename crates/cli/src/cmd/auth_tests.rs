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
        validated_at: None,
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
        validated_at: None,
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
        validated_at: None,
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
        validated_at: None,
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
        validated_at: None,
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
}

#[test]
fn auth_data_dir_falls_back_to_dot() {
    let dir = auth_data_dir();
    assert!(!dir.as_os_str().is_empty());
    assert_eq!(
        auth_data_dir_from(Err(whycodes_core::Error::Config("no home".into()))),
        std::path::PathBuf::from(".")
    );
}

fn stored(
    provider: &str,
    validated_days_ago: Option<i64>,
) -> (String, whycodes_auth::ProviderAuth) {
    let now = chrono::Utc::now();
    (
        provider.to_string(),
        whycodes_auth::ProviderAuth {
            method: "oauth".into(),
            validated_at: validated_days_ago.map(|d| now - chrono::Duration::days(d)),
            token: whycodes_auth::OAuthToken {
                access_token: "secret-access".into(),
                refresh_token: Some("secret-refresh".into()),
                expires_at: Some(now + chrono::Duration::hours(1)),
                extra: Default::default(),
            },
        },
    )
}

fn provider_config(name: &str, api_key: Option<&str>) -> whycodes_core::types::ProviderConfig {
    whycodes_core::types::ProviderConfig {
        name: name.into(),
        api_key: api_key.map(str::to_string),
        api_base: None,
        base_url: None,
        headers: None,
        models: vec![],
        tool_arguments: None,
        extra: Default::default(),
        credentials: Default::default(),
    }
}

#[test]
fn status_entries_follow_the_credential_precedence() {
    let now = chrono::Utc::now();
    let mut config = Config::default();
    config.providers.clear();
    config.providers.insert(
        "anthropic".into(),
        provider_config("anthropic", Some("cfg-key")),
    );
    config
        .providers
        .insert("openai".into(), provider_config("openai", None));
    config
        .providers
        .insert("xai".into(), provider_config("xai", Some("")));
    let mut lanes = provider_config("google", None);
    lanes
        .credentials
        .env
        .insert("ci".into(), "GOOGLE_CI_KEY".into());
    config.providers.insert("google".into(), lanes);
    let stored = vec![
        stored("anthropic", Some(1)),
        stored("github-copilot", Some(9)),
    ];
    let env = |k: &str| match k {
        "XAI_API_KEY" => Some("env-key".to_string()),
        "GOOGLE_CI_KEY" => Some("lane-key".to_string()),
        "OPENAI_API_KEY" => Some("".to_string()),
        _ => None,
    };

    let entries = auth_status_entries(&config, &stored, env, now);
    let by_name: std::collections::HashMap<&str, &AuthStatusEntry> =
        entries.iter().map(|e| (e.provider.as_str(), e)).collect();
    assert_eq!(entries.len(), 5, "{entries:?}");
    // Config api_key beats the stored login.
    let anthropic = by_name["anthropic"];
    assert_eq!(anthropic.auth_kind, "api_key");
    assert_eq!(anthropic.credential_source, "config");
    assert_eq!(anthropic.expiry, None);
    assert!(!anthropic.stale);
    // Default env var.
    assert_eq!(by_name["xai"].credential_source, "env:XAI_API_KEY");
    // Named credential lane env var.
    assert_eq!(by_name["google"].credential_source, "env:GOOGLE_CI_KEY");
    // Empty OPENAI_API_KEY and no config key: nothing.
    assert_eq!(by_name["openai"].auth_kind, "none");
    assert_eq!(by_name["openai"].credential_source, "none");
    // Store only, not renewed for over a week.
    let copilot = by_name["github-copilot"];
    assert_eq!(copilot.auth_kind, "oauth");
    assert_eq!(copilot.credential_source, "oauth_store");
    assert!(copilot.expiry.is_some());
    assert!(copilot.validated_at.is_some());
    assert!(copilot.stale);

    let json = serde_json::to_string(&entries).unwrap();
    assert!(json.contains("\"authKind\""), "{json}");
    assert!(json.contains("\"credentialSource\""), "{json}");
    assert!(!json.contains("secret-"), "{json}");
    assert!(!json.contains("cfg-key"), "{json}");

    // openai: a non-empty OPENAI_API_KEY is its fallback env var.
    let entries = auth_status_entries(
        &config,
        &[],
        |k| (k == "OPENAI_API_KEY").then(|| "env-key".to_string()),
        now,
    );
    let openai = entries.iter().find(|e| e.provider == "openai").unwrap();
    assert_eq!(openai.credential_source, "env:OPENAI_API_KEY");
    assert_eq!(openai.auth_kind, "api_key");
}

#[test]
fn oauth_expiry_prefers_the_derived_token() {
    let (_, mut auth) = stored("github-copilot", None);
    assert!(oauth_expiry(&auth).is_some());
    auth.token
        .extra
        .insert("derived_expires_at".into(), "2099-01-01T00:00:00Z".into());
    assert_eq!(oauth_expiry(&auth).as_deref(), Some("2099-01-01T00:00:00Z"));
    auth.token.extra.clear();
    auth.token.expires_at = None;
    assert_eq!(oauth_expiry(&auth), None);
}
