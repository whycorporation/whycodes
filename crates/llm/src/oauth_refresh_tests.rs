use super::*;

#[test]
fn register_unregister_roundtrip() {
    let dir = PathBuf::from("/tmp/whycodes-test-oauth-src");
    assert!(!has_source("test-provider"));
    register("test-provider", dir.clone());
    assert!(has_source("test-provider"));
    assert_eq!(source_dir("test-provider"), Some(dir));
    unregister("test-provider");
    assert!(!has_source("test-provider"));
    assert_eq!(source_dir("test-provider"), None);
}

#[test]
fn sources_are_per_provider() {
    let dir = PathBuf::from("/tmp/whycodes-test-oauth-src-2");
    register("prov-a", dir.clone());
    assert!(has_source("prov-a"));
    assert!(!has_source("prov-b"));
    unregister("prov-a");
}

#[test]
fn poisoned_sources_lock_still_registers() {
    poison_sources_for_tests();
    let dir = PathBuf::from("/tmp/whycodes-test-oauth-poison");
    register("poison-provider", dir.clone());
    assert!(has_source("poison-provider"));
    unregister("poison-provider");
}

#[test]
fn http_error_helper_formats_message() {
    let err = http_error_for_tests("dial refused");
    assert!(err.to_string().contains("HTTP error"), "{err}");
    assert!(err.to_string().contains("dial refused"), "{err}");
}

#[test]
fn auth_from_store_err_is_none() {
    assert!(auth_from_store_err_for_tests().is_none());
}

fn serve_status(status: &str, body: &str) -> String {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let payload = format!("{header}{body}");
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(payload.as_bytes());
        }
    });
    format!("http://{addr}/")
}

#[tokio::test]
async fn send_returns_success_and_401_without_refresh_source() {
    let ok_url = serve_status("200 OK", "ok");
    let ok = send_with_refresh_retry("no-such-provider", "token", |key| {
        crate::client_identity::http_client()
            .post(&ok_url)
            .bearer_auth(key)
    })
    .await
    .unwrap();
    assert_eq!(ok.status().as_u16(), 200);

    let err_url = serve_status("401 Unauthorized", "nope");
    let err = send_with_refresh_retry("no-such-provider", "token", |key| {
        crate::client_identity::http_client()
            .post(&err_url)
            .bearer_auth(key)
    })
    .await
    .unwrap();
    assert_eq!(err.status().as_u16(), 401);
    assert_eq!(err.text().await.unwrap(), "nope");
}

#[tokio::test]
async fn stored_extra_reads_oauth_map_and_force_refresh_without_token() {
    let dir = std::env::temp_dir().join(format!(
        "whycodes-oauth-extra-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::create_dir_all(&dir);
    let store = whycodes_auth::TokenStore::new(&dir);
    let mut extra = serde_json::Map::new();
    extra.insert(
        "project_id".into(),
        serde_json::Value::String("proj-from-store".into()),
    );
    extra.insert(
        "openai_account_id".into(),
        serde_json::Value::String("acct-1".into()),
    );
    store
        .set(
            "google-extra-test",
            whycodes_auth::ProviderAuth {
                method: "oauth".into(),
                token: whycodes_auth::OAuthToken {
                    access_token: "ya29.x".into(),
                    refresh_token: None,
                    expires_at: None,
                    extra,
                },
            },
        )
        .unwrap();
    register("google-extra-test", dir.clone());
    assert_eq!(
        stored_extra("google-extra-test", "project_id")
            .await
            .as_deref(),
        Some("proj-from-store")
    );
    assert_eq!(
        stored_extra("google-extra-test", "openai_account_id")
            .await
            .as_deref(),
        Some("acct-1")
    );
    assert!(stored_extra("google-extra-test", "missing").await.is_none());
    assert!(stored_extra("no-source", "project_id").await.is_none());

    let err_url = serve_status("401 Unauthorized", "nope");
    register("google-extra-test", dir.clone());
    let err = send_with_refresh_retry("google-extra-test", "ya29.x", |key| {
        crate::client_identity::http_client()
            .post(&err_url)
            .bearer_auth(key)
    })
    .await
    .unwrap();
    assert_eq!(err.status().as_u16(), 401);
    unregister("google-extra-test");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn stored_extra_none_when_store_has_no_credential() {
    let dir = std::env::temp_dir().join(format!(
        "whycodes-oauth-empty-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::create_dir_all(&dir);
    let provider = format!("empty-oauth-{}", std::process::id());
    register(&provider, dir.clone());
    assert!(stored_extra(&provider, "project_id").await.is_none());
    unregister(&provider);
    let _ = std::fs::remove_dir_all(&dir);
}

fn serve_seq(parts: Vec<(String, String)>) -> String {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for (status, body) in parts {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let header = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(format!("{header}{body}").as_bytes());
            }
        }
    });
    format!("http://{addr}/")
}

fn unique_oauth_name(label: &str) -> String {
    format!(
        "oauth-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    )
}

fn local_spec(name: String, token_url: String) -> whycodes_auth::ProviderSpec {
    whycodes_auth::ProviderSpec {
        name,
        label: "Local".into(),
        flow: whycodes_auth::FlowKind::PasteCodePkce,
        client_id: "client".into(),
        client_secret: None,
        authorize_url: token_url.clone(),
        token_url,
        scopes: "scope".into(),
        token_encoding: whycodes_auth::TokenEncoding::Form,
        redirect_uri: Some("https://example.com/callback".into()),
        loopback_port: None,
        loopback_host: None,
        callback_path: String::new(),
        extra_authorize: vec![],
        derived: None,
        suggested_models: vec![],
        inference: None,
    }
}

#[tokio::test]
async fn send_retries_once_after_force_refresh_new_token() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let name = unique_oauth_name("retry");
    let token_url = serve_status(
        "200 OK",
        r#"{"access_token":"fresh-token","token_type":"Bearer"}"#,
    );
    whycodes_auth::register_spec(local_spec(name.clone(), token_url));
    let dir = std::env::temp_dir().join(format!("whycodes-oauth-retry-{name}"));
    let _ = std::fs::create_dir_all(&dir);
    let store = whycodes_auth::TokenStore::new(&dir);
    store
        .set(
            &name,
            whycodes_auth::ProviderAuth {
                method: "oauth".into(),
                token: whycodes_auth::OAuthToken {
                    access_token: "stale-token".into(),
                    refresh_token: Some("refresh".into()),
                    expires_at: None,
                    extra: serde_json::Map::new(),
                },
            },
        )
        .unwrap();
    register(&name, dir.clone());
    let llm_url = serve_seq(vec![
        ("401 Unauthorized".into(), "nope".into()),
        ("200 OK".into(), "ok".into()),
    ]);
    let ok = send_with_refresh_retry(&name, "stale-token", |key| {
        crate::client_identity::http_client()
            .post(&llm_url)
            .bearer_auth(key)
    })
    .await
    .unwrap();
    assert_eq!(ok.status().as_u16(), 200);
    unregister(&name);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn send_returns_original_401_when_refresh_yields_same_token() {
    let name = unique_oauth_name("same");
    let token_url = serve_status(
        "200 OK",
        r#"{"access_token":"same-token","token_type":"Bearer"}"#,
    );
    whycodes_auth::register_spec(local_spec(name.clone(), token_url));
    let dir = std::env::temp_dir().join(format!("whycodes-oauth-same-{name}"));
    let _ = std::fs::create_dir_all(&dir);
    let store = whycodes_auth::TokenStore::new(&dir);
    store
        .set(
            &name,
            whycodes_auth::ProviderAuth {
                method: "oauth".into(),
                token: whycodes_auth::OAuthToken {
                    access_token: "same-token".into(),
                    refresh_token: Some("refresh".into()),
                    expires_at: None,
                    extra: serde_json::Map::new(),
                },
            },
        )
        .unwrap();
    register(&name, dir.clone());
    let err_url = serve_status("401 Unauthorized", "nope");
    let err = send_with_refresh_retry(&name, "same-token", |key| {
        crate::client_identity::http_client()
            .post(&err_url)
            .bearer_auth(key)
    })
    .await
    .unwrap();
    assert_eq!(err.status().as_u16(), 401);
    unregister(&name);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn send_maps_connect_error_and_refresh_retry_connect_error() {
    let err = send_with_refresh_retry("no-such-provider", "token", |key| {
        crate::client_identity::http_client()
            .post("http://127.0.0.1:1/")
            .bearer_auth(key)
    })
    .await
    .unwrap_err();
    assert!(err.to_string().contains("HTTP error"), "{err}");

    let name = unique_oauth_name("connect");
    let token_url = serve_status(
        "200 OK",
        r#"{"access_token":"fresh-token","token_type":"Bearer"}"#,
    );
    whycodes_auth::register_spec(local_spec(name.clone(), token_url));
    let dir = std::env::temp_dir().join(format!("whycodes-oauth-connect-{name}"));
    let _ = std::fs::create_dir_all(&dir);
    let store = whycodes_auth::TokenStore::new(&dir);
    store
        .set(
            &name,
            whycodes_auth::ProviderAuth {
                method: "oauth".into(),
                token: whycodes_auth::OAuthToken {
                    access_token: "stale-token".into(),
                    refresh_token: Some("refresh".into()),
                    expires_at: None,
                    extra: serde_json::Map::new(),
                },
            },
        )
        .unwrap();
    register(&name, dir.clone());
    let err_url = serve_status("401 Unauthorized", "nope");
    let n = std::sync::atomic::AtomicUsize::new(0);
    let err = send_with_refresh_retry(&name, "stale-token", |key| {
        let i = n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let url = if i == 0 {
            err_url.as_str()
        } else {
            "http://127.0.0.1:1/"
        };
        crate::client_identity::http_client()
            .post(url)
            .bearer_auth(key)
    })
    .await
    .unwrap_err();
    assert!(err.to_string().contains("HTTP error"), "{err}");
    unregister(&name);
    let _ = std::fs::remove_dir_all(&dir);
}
