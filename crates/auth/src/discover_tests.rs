use super::*;
use sha2::Digest as _;
use std::io::Write as _;

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(content.as_bytes()).unwrap();
}

fn found_for(source: &'static KnownSource, path: PathBuf, state: SourceState) -> FoundSource {
    FoundSource {
        source,
        path,
        state,
    }
}

#[test]
fn parses_claude_code_credentials() {
    let json = serde_json::json!({
        "claudeAiOauth": {
            "accessToken": "sk-ant-oat01-x",
            "refreshToken": "sk-ant-ort01-y",
            "expiresAt": 1735689600000i64,
            "scopes": ["user:inference"]
        }
    });
    let token = parse_claude_code(&json).unwrap();
    assert_eq!(token.access_token, "sk-ant-oat01-x");
    assert_eq!(token.refresh_token.as_deref(), Some("sk-ant-ort01-y"));
    assert!(token.expires_at.is_some());
}

#[test]
fn parse_codex_without_account_leaves_extra_empty() {
    let token = parse_codex(&serde_json::json!({
        "tokens": { "access_token": "eyJx.y.z" }
    }))
    .unwrap();
    assert!(token.extra.is_empty());
}

#[test]
fn parses_codex_auth_with_account_id() {
    let json = serde_json::json!({
        "OPENAI_API_KEY": null,
        "tokens": {
            "id_token": "ignored",
            "access_token": "eyJhbGciOiJ.eyJzdWIiOiJx.sig",
            "refresh_token": "rt",
            "account_id": "acct_123"
        }
    });
    let token = parse_codex(&json).unwrap();
    assert_eq!(token.refresh_token.as_deref(), Some("rt"));
    assert_eq!(
        token.extra.get("openai_account_id").and_then(Value::as_str),
        Some("acct_123")
    );
}

#[test]
fn parses_gemini_oauth_creds() {
    let json = serde_json::json!({
        "access_token": "ya29.abc",
        "refresh_token": "1//ref",
        "token_type": "Bearer",
        "expiry_date": 1735689600000i64
    });
    let token = parse_gemini(&json).unwrap();
    assert_eq!(token.access_token, "ya29.abc");
    assert_eq!(token.refresh_token.as_deref(), Some("1//ref"));
    assert!(token.expires_at.is_some());
}

#[test]
fn parses_copilot_hosts() {
    let json = serde_json::json!({
        "github.com": { "user": "mona", "oauth_token": "gho_abc" }
    });
    let token = parse_copilot(&json).unwrap();
    assert_eq!(token.access_token, "gho_abc");
    assert!(token.refresh_token.is_none());
    assert!(token.expires_at.is_none());
}

#[test]
fn parses_grok_build_auth_json_prefers_xai_oauth_slot() {
    let json = serde_json::json!({
        "xai::api_key": {
            "key": "xai-should-skip",
            "auth_mode": "api_key"
        },
        "https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828": {
            "key": "oauth-access",
            "auth_mode": "oidc",
            "refresh_token": "oauth-refresh",
            "expires_at": "2030-01-01T00:00:00Z"
        }
    });
    let token = parse_grok_build(&json).unwrap();
    assert_eq!(token.access_token, "oauth-access");
    assert_eq!(token.refresh_token.as_deref(), Some("oauth-refresh"));
    assert!(token.expires_at.is_some());
}

#[test]
fn parse_grok_build_skips_api_key_only_store() {
    let json = serde_json::json!({
        "xai::api_key": {
            "key": "xai-only",
            "auth_mode": "api_key"
        }
    });
    assert!(parse_grok_build(&json).is_err());
}

#[test]
fn scan_reports_without_reading() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    write(
        &home.path().join(".claude/.credentials.json"),
        r#"{"claudeAiOauth":{"accessToken":"x"}}"#,
    );
    let consent = ConsentStore::new(data.path());
    let found = scan_with_home(home.path(), &consent);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].source.provider, "anthropic");
    assert!(matches!(found[0].state, SourceState::New));
}

#[test]
fn consent_persists_across_reloads() {
    let data = tempfile::tempdir().unwrap();
    let consent = ConsentStore::new(data.path());
    let p = Path::new("/some/cred.json");
    assert_eq!(consent.decision(p), None);
    consent.record(p, true).unwrap();
    let reloaded = ConsentStore::new(data.path());
    assert_eq!(reloaded.decision(p), Some(true));
    reloaded.record(p, false).unwrap();
    assert_eq!(ConsentStore::new(data.path()).decision(p), Some(false));
}

#[test]
fn import_requires_consent() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let path = home.path().join(".gemini/oauth_creds.json");
    write(&path, r#"{"access_token":"ya29.x","refresh_token":"r"}"#);
    let consent = ConsentStore::new(data.path());
    let store = TokenStore::new(data.path());
    let found = found_for(&KNOWN_SOURCES[2], path, SourceState::New);
    let err = import(&store, &consent, &found).unwrap_err();
    assert!(matches!(err, AuthError::ConsentRequired(_)));
    assert!(store.get("google").unwrap().is_none());
}

#[cfg(unix)]
#[test]
fn symlinked_source_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let real = home.path().join("real.json");
    write(&real, r#"{"access_token":"ya29.x"}"#);
    let link = home.path().join(".gemini/oauth_creds.json");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let consent = ConsentStore::new(data.path());
    consent.record(&link, true).unwrap(); // even approved stays refused
    let found = scan_with_home(home.path(), &consent);
    assert_eq!(found.len(), 1);
    assert!(matches!(found[0].state, SourceState::Symlink));
    let store = TokenStore::new(data.path());
    let err = import(&store, &consent, &found[0]).unwrap_err();
    assert!(matches!(err, AuthError::SymlinkRejected(_)));
}

#[test]
fn import_reads_but_never_modifies_the_source() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let path = home.path().join(".codex/auth.json");
    let body = r#"{"tokens":{"access_token":"eyJx.y.z","refresh_token":"r","account_id":"a"}}"#;
    write(&path, body);
    let before_meta = std::fs::metadata(&path).unwrap();
    let before_hash = sha2::Sha256::digest(body.as_bytes());

    let consent = ConsentStore::new(data.path());
    consent.record(&path, true).unwrap();
    let store = TokenStore::new(data.path());
    let found = found_for(&KNOWN_SOURCES[1], path.clone(), SourceState::Approved);
    import(&store, &consent, &found).unwrap();

    // Stored as an imported credential.
    let auth = store.get("openai").unwrap().unwrap();
    assert_eq!(auth.method, "imported");
    assert_eq!(auth.token.access_token, "eyJx.y.z");
    assert!(auth.token.extra.contains_key("imported_from"));

    // Source untouched: same content hash, same mtime, same perms.
    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(sha2::Sha256::digest(after.as_bytes()), before_hash);
    let after_meta = std::fs::metadata(&path).unwrap();
    assert_eq!(
        before_meta.modified().unwrap(),
        after_meta.modified().unwrap()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            before_meta.permissions().mode(),
            after_meta.permissions().mode()
        );
    }
}

#[test]
fn scan_reflects_consent_decision() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let consent = ConsentStore::new(data.path());
    assert!(consent.path().ends_with("auth-consent.json"));

    let file = home.path().join(".claude/.credentials.json");
    write(&file, r#"{"claudeAiOauth":{"accessToken":"a"}}"#);
    consent.record(&file, true).unwrap();
    assert!(matches!(
        scan_with_home(home.path(), &consent)[0].state,
        SourceState::Approved
    ));
    consent.record(&file, false).unwrap();
    assert!(matches!(
        scan_with_home(home.path(), &consent)[0].state,
        SourceState::Denied
    ));
}

#[test]
fn scan_skips_directories_at_known_paths() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let consent = ConsentStore::new(data.path());
    write(
        &home.path().join(".claude/.credentials.json"),
        r#"{"claudeAiOauth":{"accessToken":"a"}}"#,
    );
    std::fs::create_dir_all(home.path().join(".codex/auth.json")).unwrap();
    assert_eq!(scan_with_home(home.path(), &consent).len(), 1);
}

#[test]
fn parse_claude_code_requires_access_token() {
    assert!(parse_claude_code(&serde_json::json!({})).is_err());
}

#[test]
fn parse_grok_build_rejects_non_object() {
    assert!(parse_grok_build(&serde_json::json!([])).is_err());
}

#[test]
fn parse_grok_build_skips_unusable_slots() {
    assert!(parse_grok_build(&serde_json::json!({"other": {"key": ""}})).is_err());
    assert!(parse_grok_build(&serde_json::json!({"other": {"value": 1}})).is_err());
    assert!(
        parse_grok_build(&serde_json::json!({
            "other": {"key": "skip", "auth_mode": "api_key"}
        }))
        .is_err()
    );
}

#[test]
fn parse_grok_build_falls_back_to_first_oauth_slot() {
    let token = parse_grok_build(&serde_json::json!({
        "other": {"key": "fallback"},
        "second": {"key": "ignored"}
    }))
    .unwrap();
    assert_eq!(token.access_token, "fallback");
}

#[test]
fn ms_to_datetime_rejects_out_of_range() {
    assert!(ms_to_datetime(i64::MAX).is_err());
}

#[test]
fn consent_read_reports_io_error() {
    let data = tempfile::tempdir().unwrap();
    let consent = ConsentStore::new(data.path());
    std::fs::create_dir(consent.path()).unwrap();
    assert!(matches!(consent.read(), Err(AuthError::Io(_))));
}

#[test]
fn consent_read_reports_json_error() {
    let data = tempfile::tempdir().unwrap();
    let consent = ConsentStore::new(data.path());
    std::fs::write(consent.path(), "not json").unwrap();
    assert!(matches!(consent.read(), Err(AuthError::Json(_))));
    assert_eq!(consent.decision(Path::new("x")), None);
}

#[test]
fn consent_record_creates_parent_directories() {
    let data = tempfile::tempdir().unwrap();
    let consent = ConsentStore::new(&data.path().join("nested/data"));
    consent.record(Path::new("x"), true).unwrap();
    assert_eq!(consent.decision(Path::new("x")), Some(true));
}

#[test]
fn scan_without_a_home_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let consent = ConsentStore::new(dir.path());
    assert!(scan_from_home(None, &consent).is_empty());
}

#[test]
fn scan_reads_home_then_userprofile_then_neither() {
    let _guard = env_lock();
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    write(
        &home.path().join(".claude/.credentials.json"),
        r#"{"claudeAiOauth":{"accessToken":"a"}}"#,
    );
    let consent = ConsentStore::new(data.path());
    let restore = EnvRestore::capture();

    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::remove_var("USERPROFILE");
    }
    assert_eq!(scan(&consent).len(), 1);

    unsafe {
        std::env::set_var("HOME", "");
        std::env::set_var("USERPROFILE", home.path());
    }
    assert_eq!(scan(&consent).len(), 1);

    unsafe {
        std::env::remove_var("HOME");
        std::env::remove_var("USERPROFILE");
    }
    assert!(scan(&consent).is_empty());

    drop(restore);
}

#[test]
fn parse_codex_falls_back_to_id_token_account() {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let payload = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&serde_json::json!({
            "https://api.openai.com/auth": {"chatgpt_account_id": "acct_jwt"}
        }))
        .unwrap(),
    );
    let json = serde_json::json!({
        "tokens": {
            "access_token": "eyJx.y.z",
            "id_token": format!("header.{payload}.sig")
        }
    });
    let token = parse_codex(&json).unwrap();
    assert_eq!(
        token.extra.get("openai_account_id").and_then(Value::as_str),
        Some("acct_jwt")
    );
}

#[test]
fn parse_expiry_rejects_out_of_range_through_parsers() {
    assert!(
        parse_claude_code(&serde_json::json!({
            "claudeAiOauth": { "accessToken": "x", "expiresAt": i64::MAX }
        }))
        .is_err()
    );
    assert!(
        parse_gemini(&serde_json::json!({
            "access_token": "x", "expiry_date": i64::MAX
        }))
        .is_err()
    );
}

#[test]
fn import_reports_io_and_json_errors() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let missing = home.path().join(".gemini/oauth_creds.json");
    let consent = ConsentStore::new(data.path());
    consent.record(&missing, true).unwrap();
    let store = TokenStore::new(data.path());
    let found = found_for(&KNOWN_SOURCES[2], missing, SourceState::Approved);
    assert!(matches!(
        import(&store, &consent, &found),
        Err(AuthError::Io(_))
    ));

    let path = home.path().join(".codex/auth.json");
    write(&path, "not json");
    consent.record(&path, true).unwrap();
    let found = found_for(&KNOWN_SOURCES[1], path, SourceState::Approved);
    assert!(matches!(
        import(&store, &consent, &found),
        Err(AuthError::Json(_))
    ));
}

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

struct EnvRestore {
    home: Option<std::ffi::OsString>,
    profile: Option<std::ffi::OsString>,
}

impl EnvRestore {
    fn capture() -> Self {
        Self {
            home: std::env::var_os("HOME"),
            profile: std::env::var_os("USERPROFILE"),
        }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        restore_os("HOME", self.home.take());
        restore_os("USERPROFILE", self.profile.take());
    }
}

fn restore_os(key: &str, prev: Option<std::ffi::OsString>) {
    match prev {
        Some(v) => unsafe { std::env::set_var(key, v) },
        None => unsafe { std::env::remove_var(key) },
    }
}
