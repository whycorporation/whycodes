use super::*;
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

const FAST: Duration = Duration::from_millis(500);

/// One HTTP/1.1 reply on a fresh loopback port; returns the `/v1` base.
fn serve_once(status: &str, body: &str, content_type: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let payload = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(payload.as_bytes());
        }
    });
    format!("http://{addr}/v1")
}

/// Accepts one connection and never answers it.
fn serve_silent() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            thread::sleep(Duration::from_secs(3));
            drop(stream);
        }
    });
    format!("http://{addr}/v1")
}

/// A port that drops every connection without answering. (Freeing a bound
/// port instead races with parallel tests that bind port 0 and reuse it.)
fn refused_base() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for stream in listener.incoming() {
            drop(stream);
        }
    });
    format!("http://{addr}/v1")
}

#[tokio::test]
async fn probe_collects_ids_from_data_array() {
    let body = json!({"data": [{"id": "qwen"}, {"id": "llama"}, {"object": "model"}]}).to_string();
    let base = serve_once("200 OK", &body, "application/json");
    let models = probe_openai_models(&base, FAST).await;
    assert_eq!(models, Some(vec!["qwen".to_string(), "llama".to_string()]));
}

#[tokio::test]
async fn probe_empty_data_array_is_found_with_no_models() {
    let base = serve_once("200 OK", r#"{"data":[]}"#, "application/json");
    assert_eq!(probe_openai_models(&base, FAST).await, Some(Vec::new()));
}

#[tokio::test]
async fn probe_rejects_non_success_status() {
    let base = serve_once("404 Not Found", r#"{"data":[]}"#, "application/json");
    assert_eq!(probe_openai_models(&base, FAST).await, None);
}

#[tokio::test]
async fn probe_rejects_non_json_body() {
    let base = serve_once("200 OK", "<html>hi</html>", "text/html");
    assert_eq!(probe_openai_models(&base, FAST).await, None);
}

#[tokio::test]
async fn probe_rejects_json_without_data_array() {
    let base = serve_once(
        "200 OK",
        r#"{"models":["x"],"data":"no"}"#,
        "application/json",
    );
    assert_eq!(probe_openai_models(&base, FAST).await, None);
}

#[tokio::test]
async fn probe_refused_port_is_not_running() {
    assert_eq!(probe_openai_models(&refused_base(), FAST).await, None);
}

#[tokio::test]
async fn probe_silent_listener_times_out() {
    let base = serve_silent();
    let started = std::time::Instant::now();
    assert_eq!(
        probe_openai_models(&base, Duration::from_millis(100)).await,
        None
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "timeout not applied"
    );
}

#[tokio::test]
async fn discover_servers_at_keeps_found_servers_in_order() {
    let first = serve_once("200 OK", r#"{"data":[{"id":"m1"}]}"#, "application/json");
    let gone = refused_base();
    let third = serve_once("200 OK", r#"{"data":[]}"#, "application/json");
    let candidates = vec![
        (LOCAL_SERVER_CANDIDATES[0], first.clone()),
        (LOCAL_SERVER_CANDIDATES[1], gone),
        (LOCAL_SERVER_CANDIDATES[2], third.clone()),
    ];
    let found = discover_servers_at(&candidates, FAST).await;
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].provider, "lmstudio");
    assert_eq!(found[0].label, "LM Studio");
    assert_eq!(found[0].api_base, first);
    assert_eq!(found[0].models, vec!["m1".to_string()]);
    assert_eq!(found[1].provider, "llamacpp");
    assert_eq!(found[1].api_base, third);
    assert!(found[1].models.is_empty());
}

#[tokio::test]
async fn discover_local_servers_only_reports_known_candidates() {
    // Whatever happens to listen on this machine, every hit maps to a candidate.
    let found = discover_local_servers(Duration::from_millis(50)).await;
    for server in &found {
        let kind = LOCAL_SERVER_CANDIDATES
            .iter()
            .find(|c| c.provider == server.provider)
            .expect("unknown provider in discovery result");
        assert_eq!(server.api_base, loopback_api_base(kind.port));
    }
    assert_eq!(loopback_api_base(1234), "http://127.0.0.1:1234/v1");
    assert_eq!(DEFAULT_DISCOVERY_TIMEOUT, Duration::from_millis(250));
}

#[test]
fn provider_config_uses_api_base_and_no_key() {
    let server = LocalServer {
        provider: "vllm".into(),
        label: "vLLM".into(),
        api_base: "http://127.0.0.1:8000/v1".into(),
        models: vec!["m".into()],
    };
    let pc = server.provider_config();
    assert_eq!(pc.name, "vllm");
    assert_eq!(pc.api_base.as_deref(), Some("http://127.0.0.1:8000/v1"));
    assert!(pc.api_key.is_none());
    assert!(pc.base_url.is_none());
    assert_eq!(pc.models, vec!["m".to_string()]);
    assert!(crate::provider_config_skips_api_key(&pc));
    let json = serde_json::to_value(&server).unwrap();
    assert_eq!(json["provider"], "vllm");
    assert_eq!(json["models"][0], "m");
}

#[test]
fn plan_save_never_overwrites_a_configured_block() {
    let server = LocalServer {
        provider: "lmstudio".into(),
        label: "LM Studio".into(),
        api_base: "http://127.0.0.1:1234/v1".into(),
        models: vec!["qwen".into()],
    };
    let LocalSave::New(fresh) = server.plan_save(None) else {
        panic!("expected New");
    };
    assert_eq!(fresh.api_base.as_deref(), Some("http://127.0.0.1:1234/v1"));

    // Same server (trailing slash / case aside): nothing to write.
    let mut same = server.provider_config();
    same.api_base = Some("HTTP://127.0.0.1:1234/v1/".into());
    same.api_key = Some("user-key".into());
    assert!(matches!(
        server.plan_save(Some(&same)),
        LocalSave::AlreadyConfigured
    ));

    // `base_url` wins over `api_base`, as in the provider registry.
    let mut via_base_url = server.provider_config();
    via_base_url.api_base = None;
    via_base_url.base_url = Some("http://127.0.0.1:1234/v1".into());
    assert!(matches!(
        server.plan_save(Some(&via_base_url)),
        LocalSave::AlreadyConfigured
    ));

    // Elsewhere: untouched, reported.
    let mut other = server.provider_config();
    other.api_base = Some("http://10.0.0.5:1234/v1".into());
    let LocalSave::Conflict { configured } = server.plan_save(Some(&other)) else {
        panic!("expected Conflict");
    };
    assert_eq!(configured, "http://10.0.0.5:1234/v1");

    // No base at all: only api_base is filled, the rest is kept.
    let mut bare = server.provider_config();
    bare.api_base = None;
    bare.api_key = Some("user-key".into());
    bare.models = vec!["mine".into()];
    let LocalSave::FillApiBase(filled) = server.plan_save(Some(&bare)) else {
        panic!("expected FillApiBase");
    };
    assert_eq!(filled.api_base.as_deref(), Some("http://127.0.0.1:1234/v1"));
    assert_eq!(filled.api_key.as_deref(), Some("user-key"));
    assert_eq!(filled.models, vec!["mine".to_string()]);
}
