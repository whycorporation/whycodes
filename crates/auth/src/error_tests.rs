use super::*;

#[test]
fn variants_display_without_secrets() {
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
