use super::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Form, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

/// Knobs for the mock MCP + authorization server.
#[derive(Clone)]
struct Mock {
    base: String,
    /// Publish protected-resource metadata (and hint it in the 401).
    prm: bool,
    registration: bool,
    register_ok: bool,
    secret: bool,
    refresh_ok: bool,
    token_json: bool,
    grants: Arc<Mutex<Vec<HashMap<String, String>>>>,
}

impl Mock {
    fn new() -> Self {
        Self {
            base: String::new(),
            prm: true,
            registration: true,
            register_ok: true,
            secret: false,
            refresh_ok: true,
            token_json: true,
            grants: Arc::default(),
        }
    }
}

async fn mcp(State(m): State<Mock>) -> Response {
    let challenge = if m.prm {
        format!("Bearer resource_metadata=\"{}/meta/prm\"", m.base)
    } else {
        "Bearer realm=\"mcp\"".to_string()
    };
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, challenge)],
    )
        .into_response()
}

async fn prm(State(m): State<Mock>) -> Response {
    if !m.prm {
        return StatusCode::NOT_FOUND.into_response();
    }
    Json(json!({
        "resource": format!("{}/mcp", m.base),
        "authorization_servers": [format!("{}/as", m.base)],
        "scopes_supported": ["mcp.read"],
    }))
    .into_response()
}

fn as_metadata(m: &Mock) -> Value {
    let mut meta = json!({
        "authorization_endpoint": format!("{}/authorize", m.base),
        "token_endpoint": format!("{}/token", m.base),
        "scopes_supported": ["origin.scope"],
    });
    if m.registration {
        meta["registration_endpoint"] = json!(format!("{}/register", m.base));
    }
    meta
}

async fn as_meta_issuer(State(m): State<Mock>) -> Response {
    Json(as_metadata(&m)).into_response()
}

async fn as_meta_origin(State(m): State<Mock>) -> Response {
    if m.prm {
        return StatusCode::NOT_FOUND.into_response();
    }
    Json(as_metadata(&m)).into_response()
}

async fn register(State(m): State<Mock>) -> Response {
    if !m.register_ok {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "nope"}))).into_response();
    }
    let mut body = json!({"client_id": "cid"});
    if m.secret {
        body["client_secret"] = json!("shh");
    }
    (StatusCode::CREATED, Json(body)).into_response()
}

async fn token(State(m): State<Mock>, Form(form): Form<HashMap<String, String>>) -> Response {
    m.grants.lock().unwrap().push(form.clone());
    if !m.token_json {
        return "not json".into_response();
    }
    let grant = form.get("grant_type").map(String::as_str);
    match grant {
        Some("authorization_code") if form.get("code").map(String::as_str) == Some("good") => {
            Json(json!({"access_token": "at1", "refresh_token": "rt1", "expires_in": 3600}))
                .into_response()
        }
        Some("refresh_token") if m.refresh_ok => {
            Json(json!({"access_token": "at2", "expires_in": 3600})).into_response()
        }
        _ => (StatusCode::BAD_REQUEST, "invalid_grant").into_response(),
    }
}

async fn start(mut mock: Mock) -> Mock {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    mock.base = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/mcp", post(mcp))
        .route("/meta/prm", get(prm))
        .route(
            "/.well-known/oauth-authorization-server/as",
            get(as_meta_issuer),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(as_meta_origin),
        )
        .route("/register", post(register))
        .route("/token", post(token))
        .with_state(mock.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    mock
}

/// An `open` callback that plays the browser: redirect back with `code`.
fn browser(code: &'static str, wrong_state: bool, seen: Arc<Mutex<String>>) -> impl FnOnce(&str) {
    move |url: &str| {
        *seen.lock().unwrap() = url.to_string();
        let query: HashMap<String, String> =
            url::form_urlencoded::parse(url.split_once('?').unwrap().1.as_bytes())
                .into_owned()
                .collect();
        let state = if wrong_state {
            "forged".to_string()
        } else {
            query["state"].clone()
        };
        let back = format!("{}?code={code}&state={state}", query["redirect_uri"]);
        tokio::spawn(async move { reqwest::get(back).await.unwrap() });
    }
}

fn stored(dir: &Path) -> ProviderAuth {
    store(dir).get(&key("docs")).unwrap().unwrap()
}

fn expire(dir: &Path) {
    let mut auth = stored(dir);
    auth.token.expires_at = Some(chrono::Utc::now() - chrono::Duration::seconds(5));
    store(dir).set(&key("docs"), auth).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn login_stores_a_token_that_refreshes() {
    let mut mock = Mock::new();
    mock.secret = true;
    let mock = start(mock).await;
    let dir = tempfile::tempdir().unwrap();
    let seen = Arc::new(Mutex::new(String::new()));
    let url = format!("{}/mcp", mock.base);
    login(
        "docs",
        &url,
        dir.path(),
        browser("good", false, seen.clone()),
    )
    .await
    .unwrap();

    let authorize = seen.lock().unwrap().clone();
    assert!(
        authorize.starts_with(&format!("{}/authorize?", mock.base)),
        "{authorize}"
    );
    assert!(authorize.contains("client_id=cid"), "{authorize}");
    assert!(authorize.contains("scope=mcp.read"), "{authorize}");
    assert!(authorize.contains("resource=http"), "{authorize}");
    let exchange = mock.grants.lock().unwrap()[0].clone();
    assert_eq!(exchange["client_secret"], "shh");
    assert_eq!(exchange["resource"], url);
    assert!(!exchange["code_verifier"].is_empty());

    assert_eq!(bearer("docs", dir.path()).await.as_deref(), Some("at1"));
    expire(dir.path());
    assert_eq!(bearer("docs", dir.path()).await.as_deref(), Some("at2"));
    let renewed = stored(dir.path());
    assert_eq!(renewed.token.refresh_token.as_deref(), Some("rt1"), "kept");
    let refresh = mock.grants.lock().unwrap()[1].clone();
    assert_eq!(refresh["grant_type"], "refresh_token");
    assert_eq!(refresh["client_secret"], "shh");

    assert!(logout("docs", dir.path()).unwrap());
    assert!(!logout("docs", dir.path()).unwrap());
    assert_eq!(bearer("docs", dir.path()).await, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn refresh_failures_leave_no_token() {
    let mut mock = Mock::new();
    mock.refresh_ok = false;
    let mock = start(mock).await;
    let dir = tempfile::tempdir().unwrap();
    let url = format!("{}/mcp", mock.base);
    let seen = Arc::new(Mutex::new(String::new()));
    login("docs", &url, dir.path(), browser("good", false, seen))
        .await
        .unwrap();
    expire(dir.path());
    assert_eq!(bearer("docs", dir.path()).await, None, "refresh rejected");

    // No refresh token at all.
    let mut auth = stored(dir.path());
    auth.token.refresh_token = None;
    store(dir.path()).set(&key("docs"), auth).unwrap();
    assert_eq!(bearer("docs", dir.path()).await, None);

    // No expiry: used as is.
    let mut auth = stored(dir.path());
    auth.token.expires_at = None;
    store(dir.path()).set(&key("docs"), auth).unwrap();
    assert_eq!(bearer("docs", dir.path()).await.as_deref(), Some("at1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn refreshed_token_is_returned_even_if_saving_fails() {
    let mock = start(Mock::new()).await;
    let dir = tempfile::tempdir().unwrap();
    let seen = Arc::new(Mutex::new(String::new()));
    login(
        "docs",
        &format!("{}/mcp", mock.base),
        dir.path(),
        browser("good", false, seen),
    )
    .await
    .unwrap();
    expire(dir.path());
    // The store writes through `auth.json.tmp`; a directory there breaks it.
    std::fs::create_dir(dir.path().join("mcp").join("auth.json.tmp")).unwrap();
    assert_eq!(bearer("docs", dir.path()).await.as_deref(), Some("at2"));
}

#[tokio::test]
async fn unreadable_store_means_no_token() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("mcp")).unwrap();
    std::fs::write(dir.path().join("mcp").join("auth.json"), "{broken").unwrap();
    assert_eq!(bearer("docs", dir.path()).await, None);
    assert!(logout("docs", dir.path()).is_err());
}

#[tokio::test]
async fn discovery_falls_back_to_the_server_origin() {
    let mut mock = Mock::new();
    mock.prm = false;
    let mock = start(mock).await;
    let http = reqwest::Client::new();
    let url = format!("{}/mcp", mock.base);
    let found = discover(&http, &url).await.unwrap();
    assert_eq!(found.resource, url);
    assert_eq!(found.scopes, ["origin.scope"]);
    assert_eq!(
        found.metadata.token_endpoint,
        format!("{}/token", mock.base)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn login_errors() {
    let dir = tempfile::tempdir().unwrap();
    let seen = || Arc::new(Mutex::new(String::new()));

    let bad_url = login("docs", "not a url", dir.path(), |_| {}).await;
    assert!(bad_url.unwrap_err().to_string().contains("invalid URL"));
    // Nothing listening: discovery finds no metadata.
    let closed = login("docs", "http://127.0.0.1:9/mcp", dir.path(), |_| {}).await;
    assert!(
        closed
            .unwrap_err()
            .to_string()
            .contains("no OAuth authorization server metadata")
    );

    let mut no_dcr = Mock::new();
    no_dcr.registration = false;
    let no_dcr = start(no_dcr).await;
    let err = login("docs", &format!("{}/mcp", no_dcr.base), dir.path(), |_| {})
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("dynamic client registration"),
        "{err}"
    );

    let mut refused = Mock::new();
    refused.register_ok = false;
    let refused = start(refused).await;
    let err = login("docs", &format!("{}/mcp", refused.base), dir.path(), |_| {})
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("client registration returned 400"),
        "{err}"
    );

    let mock = start(Mock::new()).await;
    let url = format!("{}/mcp", mock.base);
    let err = login("docs", &url, dir.path(), browser("bad", false, seen()))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("token endpoint returned 400"),
        "{err}"
    );
    let err = login("docs", &url, dir.path(), browser("good", true, seen()))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("state mismatch"), "{err}");

    let mut garbled = Mock::new();
    garbled.token_json = false;
    let garbled = start(garbled).await;
    let err = login(
        "docs",
        &format!("{}/mcp", garbled.base),
        dir.path(),
        browser("good", false, seen()),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("invalid token response"), "{err}");
    assert!(store(dir.path()).get(&key("docs")).unwrap().is_none());
}

#[tokio::test]
async fn unreachable_endpoints_report_the_step() {
    let http = reqwest::Client::new();
    let err = register_client(&http, "http://127.0.0.1:9/register", "http://x")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("client registration failed"),
        "{err}"
    );
    let err = token_request(&http, "http://127.0.0.1:9/token", &[])
        .await
        .err()
        .unwrap();
    assert!(err.to_string().contains("token request failed"), "{err}");
}

#[tokio::test]
async fn callback_join_errors_are_reported() {
    let task = tokio::spawn(tokio::time::sleep(Duration::from_secs(3600)));
    task.abort();
    let err = callback_result(Err(task.await.unwrap_err())).err().unwrap();
    assert!(
        err.to_string().contains("OAuth callback wait failed"),
        "{err}"
    );
}

#[test]
fn resource_metadata_from_challenges() {
    assert_eq!(
        resource_metadata_url(r#"Bearer realm="x", resource_metadata="https://a/prm""#).as_deref(),
        Some("https://a/prm")
    );
    assert_eq!(
        resource_metadata_url("Bearer resource_metadata=https://a/prm, error=x").as_deref(),
        Some("https://a/prm")
    );
    assert_eq!(
        resource_metadata_url(r#"Bearer resource_metadata="""#),
        None
    );
    assert_eq!(resource_metadata_url("Bearer realm=x"), None);
}
