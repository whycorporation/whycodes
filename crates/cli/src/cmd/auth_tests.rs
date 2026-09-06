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
    assert!(imported_count_line(2).contains("2"));
}
