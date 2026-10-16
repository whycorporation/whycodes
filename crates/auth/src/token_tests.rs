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

#[test]
fn stale_after_a_week_without_renewal() {
    let now = Utc::now();
    let week_and_a_bit = Duration::days(7) + Duration::hours(1);
    let mut auth = ProviderAuth {
        method: "oauth".to_string(),
        validated_at: Some(now - week_and_a_bit),
        token: token(None, None),
    };
    assert!(auth.is_stale_at(now));
    auth.validated_at = Some(now - Duration::days(6));
    assert!(!auth.is_stale_at(now));

    // No validation stamp: an access token that expired a week ago is stale,
    // one that expired yesterday is not, and no evidence at all is not.
    auth.validated_at = None;
    auth.token.expires_at = Some(now - week_and_a_bit);
    assert!(auth.is_stale_at(now));
    auth.token.expires_at = Some(now - Duration::days(1));
    assert!(!auth.is_stale_at(now));
    auth.token.expires_at = None;
    assert!(!auth.is_stale_at(now));
}

#[test]
fn validated_at_is_optional_on_disk() {
    let legacy: ProviderAuth =
        serde_json::from_str(r#"{"method":"oauth","token":{"access_token":"a"}}"#).unwrap();
    assert!(legacy.validated_at.is_none());
    let json = serde_json::to_string(&legacy).unwrap();
    assert!(!json.contains("validated_at"), "{json}");
    let stamped = ProviderAuth {
        validated_at: Some(Utc::now()),
        ..legacy
    };
    let json = serde_json::to_string(&stamped).unwrap();
    assert!(json.contains("validated_at"), "{json}");
}
