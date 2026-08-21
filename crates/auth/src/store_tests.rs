use super::*;
use crate::token::OAuthToken;

fn token() -> ProviderAuth {
    ProviderAuth {
        method: "oauth".to_string(),
        token: OAuthToken {
            access_token: "acc".to_string(),
            refresh_token: Some("ref".to_string()),
            expires_at: None,
            extra: Default::default(),
        },
    }
}

#[test]
fn roundtrip_set_get_remove() {
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path());
    assert!(store.get("anthropic").unwrap().is_none());
    store.set("anthropic", token()).unwrap();
    assert!(store.get("anthropic").unwrap().is_some());
    assert!(store.remove("anthropic").unwrap());
    assert!(store.get("anthropic").unwrap().is_none());
    assert!(!store.remove("anthropic").unwrap());
    store.set("openai", token()).unwrap();
    store.set("anthropic", token()).unwrap();
    let listed = store.list().unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].0, "anthropic");
    assert!(store.path().ends_with("auth.json"));
}

#[test]
fn reports_io_and_json_errors() {
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path());
    std::fs::create_dir(store.path()).unwrap();
    assert!(matches!(store.get("openai"), Err(AuthError::Io(_))));

    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path());
    std::fs::write(store.path(), "not json").unwrap();
    #[cfg(unix)]
    set_owner_only(store.path()).unwrap();
    assert!(matches!(store.get("openai"), Err(AuthError::Json(_))));

    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(&dir.path().join("nested/data"));
    store.set("openai", token()).unwrap();
    assert!(store.path().is_file());
}

#[cfg(unix)]
#[test]
fn store_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path());
    store.set("openai", token()).unwrap();
    let mode = std::fs::metadata(store.path())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}

#[cfg(unix)]
#[test]
fn refuses_world_readable_store() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path());
    store.set("openai", token()).unwrap();
    std::fs::set_permissions(store.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
    let err = store.get("openai").unwrap_err();
    assert!(matches!(err, AuthError::InsecureStorePermissions(_)));
}
