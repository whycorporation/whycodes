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
