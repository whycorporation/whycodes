use super::*;

fn token(expires_at: Option<DateTime<Utc>>, refresh: Option<&str>) -> OAuthToken {
    OAuthToken {
        access_token: "secret-access".to_string(),
        refresh_token: refresh.map(str::to_string),
        expires_at,
        extra: serde_json::Map::from_iter([(
            "project".to_string(),
            serde_json::Value::String("secret-extra".to_string()),
        )]),
    }
}

#[test]
fn expiry_refresh_and_debug_are_safe() {
    let fresh = token(
        Some(Utc::now() + Duration::minutes(5)),
        Some("secret-refresh"),
    );
    assert!(!fresh.is_expired());
    assert!(fresh.can_refresh());
    let debug = format!("{fresh:?}");
    assert!(debug.contains("<redacted>"));
    assert!(debug.contains("project"));
    assert!(!debug.contains("secret-access"));
    assert!(!debug.contains("secret-refresh"));
    assert!(!debug.contains("secret-extra"));

    assert!(token(Some(Utc::now()), None).is_expired());
    let no_expiry = token(None, None);
    assert!(!no_expiry.is_expired());
    assert!(!no_expiry.can_refresh());
    assert!(format!("{no_expiry:?}").contains("refresh_token: None"));
}
