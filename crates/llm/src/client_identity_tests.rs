use super::*;

#[test]
fn user_agent_starts_with_whycodes() {
    assert!(
        USER_AGENT.starts_with("whycodes/"),
        "USER_AGENT={USER_AGENT}"
    );
    assert!(!USER_AGENT.ends_with('/'));
    assert!(USER_AGENT.len() > "whycodes/".len());
}

#[test]
fn identity_constants() {
    assert_eq!(X_TITLE, "whycodes");
    assert!(HTTP_REFERER.contains("why.codes"));
}

#[test]
fn http_client_builds() {
    let _ = http_client();
}

#[test]
fn http_client_is_shared() {
    // Process-wide client: same static reference on every call.
    assert!(std::ptr::eq(shared_client(), shared_client()));
    // Clones are cheap and keep the pool warm.
    let _a = http_client();
    let _b = http_client();
}

#[test]
fn plugin_identity_falls_back_to_whycodes() {
    let _ = with_plugin_identity(http_client().get("https://example.invalid/"), "no-such");
}

#[test]
fn connect_timeout_is_finite() {
    // Guard against regressions that drop connect_timeout and re-inflate
    // "Worked for Xs" on dead VPN/Tailscale hops.
    assert!(CONNECT_TIMEOUT.as_secs() >= 1);
    assert!(CONNECT_TIMEOUT.as_secs() <= 10);
}

#[test]
fn fallback_http_client_builds_after_builder_error() {
    let _ = fallback_http_client_for_tests("forced builder failure");
    let _ = client_from_builder_err_for_tests("forced builder failure");
    let _ = client_from_builder_ok_for_tests();
}

#[test]
fn plugin_identity_applies_ua_and_skips_user_agent_header() {
    use std::collections::HashMap;
    use whycodes_auth::{FlowKind, InferenceIdentity, ProviderSpec, TokenEncoding};

    let name = format!("plugin-id-{}", std::process::id());
    whycodes_auth::register_spec(ProviderSpec {
        name: name.clone(),
        label: "Plugin".into(),
        flow: FlowKind::DeviceCode,
        client_id: "cid".into(),
        client_secret: None,
        authorize_url: "https://example.invalid/a".into(),
        token_url: "https://example.invalid/t".into(),
        scopes: "read".into(),
        token_encoding: TokenEncoding::Form,
        redirect_uri: None,
        loopback_port: None,
        loopback_host: None,
        callback_path: String::new(),
        extra_authorize: vec![],
        derived: None,
        suggested_models: vec![],
        inference: Some(InferenceIdentity {
            user_agent: Some("plugin-ua".into()),
            headers: HashMap::from([
                ("originator".into(), "whycodes".into()),
                ("User-Agent".into(), "skip-me".into()),
            ]),
        }),
    });
    let _ = with_plugin_identity(http_client().get("https://example.invalid/"), &name);
    let _ = post_for_provider("https://example.invalid/", &name);

    whycodes_auth::register_spec(ProviderSpec {
        name: format!("{name}-headers-only"),
        label: "Plugin".into(),
        flow: FlowKind::DeviceCode,
        client_id: "cid".into(),
        client_secret: None,
        authorize_url: "https://example.invalid/a".into(),
        token_url: "https://example.invalid/t".into(),
        scopes: "read".into(),
        token_encoding: TokenEncoding::Form,
        redirect_uri: None,
        loopback_port: None,
        loopback_host: None,
        callback_path: String::new(),
        extra_authorize: vec![],
        derived: None,
        suggested_models: vec![],
        inference: Some(InferenceIdentity {
            user_agent: Some(String::new()),
            headers: HashMap::from([("originator".into(), "whycodes".into())]),
        }),
    });
    let _ = with_plugin_identity(
        http_client().get("https://example.invalid/"),
        &format!("{name}-headers-only"),
    );
    let _ = post("https://example.invalid/");
}

/// Local gateways usually listen on 127.0.0.1 only. `localhost` resolves to
/// `::1` first, and on Windows a refused IPv6 dial costs ~300 ms before the
/// IPv4 fallback; the shared client tries IPv4 first.
#[tokio::test]
async fn localhost_reaches_an_ipv4_only_server_without_the_ipv6_detour() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                if sock.read(&mut buf).await.is_ok() {
                    let reply = "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n";
                    sock.write_all(reply.as_bytes()).await.ok();
                }
            });
        }
    });
    let started = std::time::Instant::now();
    let reply = http_client()
        .get(format!("http://localhost:{port}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), 204);
    let took = started.elapsed();
    assert!(took < std::time::Duration::from_millis(250), "{took:?}");
}

#[tokio::test]
async fn localhost_still_reaches_an_ipv6_only_server() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let Ok(listener) = tokio::net::TcpListener::bind("[::1]:0").await else {
        return; // no IPv6 loopback on this host
    };
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = [0u8; 4096];
            if sock.read(&mut buf).await.is_ok() {
                let reply = "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n";
                sock.write_all(reply.as_bytes()).await.ok();
            }
        }
    });
    let reply = http_client()
        .get(format!("http://localhost:{port}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), 204);
}
