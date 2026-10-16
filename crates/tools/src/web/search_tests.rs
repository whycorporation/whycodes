use super::*;

#[test]
fn strip_markup_removes_nested_bold() {
    let s = "Preparing for <b>Nuxt</b> 5";
    let out = strip_markup(s);
    assert!(!out.contains("<b>"));
    assert!(out.contains("Nuxt"));
    assert!(out.contains("5"));
}

#[tokio::test]
async fn execute_requires_query() {
    let ctx = crate::tool::ToolContext::new("/");
    let out = WebSearchTool::new()
        .execute(serde_json::json!({}), &ctx)
        .await;
    assert!(out.is_error, "{}", out.content);
    assert!(out.content.contains("Query is required"), "{}", out.content);
}

struct SearchEnv {
    _lock: std::sync::MutexGuard<'static, ()>,
    prev_key: Option<std::ffi::OsString>,
    prev_serp: Option<std::ffi::OsString>,
    prev_ddg: Option<std::ffi::OsString>,
}

impl SearchEnv {
    fn lock() -> Self {
        let lock = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        Self {
            _lock: lock,
            prev_key: std::env::var_os("SERPAPI_API_KEY"),
            prev_serp: std::env::var_os("WHYCODES_SERPAPI_BASE"),
            prev_ddg: std::env::var_os("WHYCODES_DDG_BASE"),
        }
    }
}

impl Drop for SearchEnv {
    fn drop(&mut self) {
        unsafe {
            match &self.prev_key {
                Some(v) => std::env::set_var("SERPAPI_API_KEY", v),
                None => std::env::remove_var("SERPAPI_API_KEY"),
            }
            match &self.prev_serp {
                Some(v) => std::env::set_var("WHYCODES_SERPAPI_BASE", v),
                None => std::env::remove_var("WHYCODES_SERPAPI_BASE"),
            }
            match &self.prev_ddg {
                Some(v) => std::env::set_var("WHYCODES_DDG_BASE", v),
                None => std::env::remove_var("WHYCODES_DDG_BASE"),
            }
        }
    }
}

fn spawn_http(body: &str, content_type: &str, n: usize) -> std::net::SocketAddr {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let payload = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    std::thread::spawn(move || {
        for _ in 0..n {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(payload.as_bytes());
            }
        }
    });
    addr
}

#[test]
fn urlencoding_and_plain_markup() {
    assert_eq!(urlencoding("a b"), "a+b");
    assert_eq!(urlencoding("A-Z_0~."), "A-Z_0~.");
    assert!(urlencoding("/").contains("%2F"));
    assert_eq!(strip_markup(" plain "), "plain");
    let t = WebSearchTool::default();
    assert_eq!(t.name(), "websearch");
    assert_eq!(WebSearchTool::new().name(), "websearch");
    assert!(!t.description().is_empty());
    assert_eq!(t.parameters()["required"][0], "query");
    assert_eq!(
        search_host_with("MISSING_SEARCH_HOST", "default.host", false),
        "default.host"
    );
    note_no_serpapi(&std::env::VarError::NotPresent);
    note_search_url_ok();
    note_no_organic();
    assert_eq!(search_body_error("eof"), "eof");
    let read_err = search_read_error("eof");
    assert!(read_err.is_error);
    assert!(read_err.content.contains("Error reading response"));
    assert!(search_text_failed("eof").is_error);
    assert!(search_html_result(Err("eof".into()), 3).is_error);
    let empty = search_html_result(Ok("<div>nope</div>".into()), 3);
    assert!(!empty.is_error);
    assert!(empty.content.contains("No results"));
    let hits = search_html_result(Ok("<div class=\"result__snippet\">alpha</div>".into()), 3);
    assert!(hits.content.contains("alpha"));
    let collapsed = search_html_result(Ok("<div class=\"result__snippet\"></div>".into()), 3);
    assert!(
        collapsed.content.contains("No results"),
        "{}",
        collapsed.content
    );
}

#[tokio::test]
async fn serpapi_loopback_parses_results_and_empty() {
    let _env = SearchEnv::lock();
    let body = r#"{"organic_results":[{"title":"<b>Hi</b>","link":"http://x","snippet":"s"}]}"#;
    let addr = spawn_http(body, "application/json", 1);
    unsafe {
        std::env::set_var("SERPAPI_API_KEY", "k");
        std::env::set_var("WHYCODES_SERPAPI_BASE", format!("http://{addr}"));
    }
    let out = WebSearchTool::new()
        .execute(
            serde_json::json!({"query": "hi", "num_results": 3}),
            &crate::tool::ToolContext::unsandboxed("/"),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("Hi"), "{}", out.content);

    let addr = spawn_http("{}", "application/json", 1);
    unsafe {
        std::env::set_var("WHYCODES_SERPAPI_BASE", format!("http://{addr}"));
    }
    let empty = WebSearchTool::new()
        .execute(
            serde_json::json!({"query": "hi"}),
            &crate::tool::ToolContext::unsandboxed("/"),
        )
        .await;
    assert!(!empty.is_error, "{}", empty.content);
    assert!(empty.content.contains("No results"), "{}", empty.content);

    let addr = spawn_http("not-json", "text/plain", 1);
    unsafe {
        std::env::set_var("WHYCODES_SERPAPI_BASE", format!("http://{addr}"));
    }
    let bad = WebSearchTool::new()
        .execute(
            serde_json::json!({"query": "hi"}),
            &crate::tool::ToolContext::unsandboxed("/"),
        )
        .await;
    assert!(bad.is_error, "{}", bad.content);
}

#[tokio::test]
async fn duckduckgo_fallback_extracts_snippets() {
    let _env = SearchEnv::lock();
    unsafe { std::env::remove_var("SERPAPI_API_KEY") };
    let html = "<div class=\"result__snippet\">alpha &amp; beta</div>\n<div class=\"result__snippet\">gamma</div>\n";
    let addr = spawn_http(html, "text/html", 1);
    unsafe {
        std::env::set_var("WHYCODES_DDG_BASE", format!("http://{addr}"));
    }
    let out = WebSearchTool::new()
        .execute(
            serde_json::json!({"query": "q", "num_results": 1}),
            &crate::tool::ToolContext::unsandboxed("/"),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("alpha"), "{}", out.content);

    let addr = spawn_http("<html>no snippets</html>", "text/html", 1);
    unsafe {
        std::env::set_var("WHYCODES_DDG_BASE", format!("http://{addr}"));
    }
    let empty = WebSearchTool::new()
        .execute(
            serde_json::json!({"query": "q"}),
            &crate::tool::ToolContext::unsandboxed("/"),
        )
        .await;
    assert!(!empty.is_error, "{}", empty.content);
    assert!(
        empty.content.contains("SERPAPI_API_KEY"),
        "{}",
        empty.content
    );

    let mut ctx = crate::tool::ToolContext::unsandboxed("/");
    ctx.network = whycodes_core::NetworkPolicy {
        allowlist: vec!["example.com".into()],
        denylist: vec![],
    };
    let blocked = WebSearchTool::new()
        .execute(serde_json::json!({"query": "q"}), &ctx)
        .await;
    assert!(blocked.is_error, "{}", blocked.content);
}

#[tokio::test]
async fn serpapi_and_ddg_network_and_connect_errors() {
    let _env = SearchEnv::lock();
    unsafe {
        std::env::set_var("SERPAPI_API_KEY", "k");
        std::env::set_var("WHYCODES_SERPAPI_BASE", "http://127.0.0.1:1");
    }
    let mut ctx = crate::tool::ToolContext::unsandboxed("/");
    ctx.network = whycodes_core::NetworkPolicy {
        allowlist: vec!["example.com".into()],
        denylist: vec![],
    };
    let blocked = WebSearchTool::new()
        .execute(serde_json::json!({"query": "q"}), &ctx)
        .await;
    assert!(blocked.is_error, "{}", blocked.content);

    let connect = WebSearchTool::new()
        .execute(
            serde_json::json!({"query": "q"}),
            &crate::tool::ToolContext::unsandboxed("/"),
        )
        .await;
    assert!(connect.is_error, "{}", connect.content);
    assert!(
        connect.content.contains("Error performing search"),
        "{}",
        connect.content
    );

    unsafe { std::env::remove_var("SERPAPI_API_KEY") };
    unsafe { std::env::set_var("WHYCODES_DDG_BASE", "http://127.0.0.1:1") };
    let ddg = WebSearchTool::new()
        .execute(
            serde_json::json!({"query": "q"}),
            &crate::tool::ToolContext::unsandboxed("/"),
        )
        .await;
    assert!(ddg.is_error, "{}", ddg.content);
}

#[tokio::test]
async fn duckduckgo_network_deny_without_serpapi() {
    let _env = SearchEnv::lock();
    unsafe { std::env::remove_var("SERPAPI_API_KEY") };
    let mut ctx = crate::tool::ToolContext::unsandboxed("/");
    ctx.network = whycodes_core::NetworkPolicy {
        allowlist: vec!["example.com".into()],
        denylist: vec![],
    };
    let blocked = WebSearchTool::new()
        .execute(serde_json::json!({"query": "q"}), &ctx)
        .await;
    assert!(blocked.is_error, "{}", blocked.content);
}

/// Loopback stub that answers `n` requests with `status` + `body` and hands
/// back each raw request (head + body) for assertions.
fn spawn_recording(
    status: &str,
    body: &str,
    n: usize,
) -> (std::net::SocketAddr, std::sync::mpsc::Receiver<String>) {
    use std::io::Write;
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let payload = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nLocation: http://127.0.0.1:1/x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for _ in 0..n {
            if let Ok((mut stream, _)) = listener.accept() {
                let req = read_request(&mut stream);
                let _ = tx.send(req);
                let _ = stream.write_all(payload.as_bytes());
            }
        }
    });
    (addr, rx)
}

/// Read one request: the head, then `Content-Length` bytes of body.
fn read_request(stream: &mut std::net::TcpStream) -> String {
    use std::io::Read;
    let mut req = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let got = stream.read(&mut buf).unwrap_or(0);
        req.extend_from_slice(&buf[..got]);
        let text = String::from_utf8_lossy(&req).to_string();
        let complete = text.split_once("\r\n\r\n").is_some_and(|(head, rest)| {
            let len = head
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                })
                .unwrap_or(0);
            rest.len() >= len
        });
        if got == 0 || complete {
            return text;
        }
    }
}

fn custom_settings(endpoint: String) -> CustomSearchSettings {
    CustomSearchSettings {
        endpoint,
        method: "GET".into(),
        query_param: "q".into(),
        count_param: Some("n".into()),
        params: [("lang".to_string(), "en".to_string())].into(),
        headers: [("Accept".to_string(), "application/json".to_string())].into(),
        header_env: Default::default(),
        results_pointer: "/results".into(),
        title_pointer: "/title".into(),
        url_pointer: "/url".into(),
        snippet_pointer: "/snippet".into(),
    }
}

fn custom_tool(custom: CustomSearchSettings, fallback: &[&str]) -> WebSearchTool {
    let tool = WebSearchTool::new();
    tool.set_settings(WebSearchSettings {
        engine: Some("Custom".into()),
        fallback: fallback.iter().map(|s| s.to_string()).collect(),
        custom: Some(custom),
    });
    tool
}

async fn run_custom(custom: CustomSearchSettings) -> ToolResult {
    custom_tool(custom, &[])
        .execute(
            serde_json::json!({"query": "q"}),
            &crate::tool::ToolContext::unsandboxed("/"),
        )
        .await
}

const CUSTOM_BODY: &str = r#"{"results":[{"title":"<b>One</b>","url":"https://one.example","snippet":"first"},{"title":"Two","url":"https://two.example"},{"title":"Three","url":"https://three.example"}]}"#;

#[tokio::test]
async fn custom_get_maps_results_and_sends_params_and_env_header() {
    let _env = SearchEnv::lock();
    unsafe { std::env::set_var("WHYCODES_TEST_SEARCH_KEY", "sekrit") };
    let (addr, rx) = spawn_recording("200 OK", CUSTOM_BODY, 1);
    let mut custom = custom_settings(format!("http://{addr}/api"));
    custom.header_env = [(
        "X-Api-Key".to_string(),
        "WHYCODES_TEST_SEARCH_KEY".to_string(),
    )]
    .into();
    let out = custom_tool(custom, &[])
        .execute(
            serde_json::json!({"query": "rust lang", "num_results": 2}),
            &crate::tool::ToolContext::unsandboxed("/"),
        )
        .await;
    unsafe { std::env::remove_var("WHYCODES_TEST_SEARCH_KEY") };
    assert!(!out.is_error, "{}", out.content);
    assert!(
        out.content
            .contains("1. One\n   https://one.example\n   first"),
        "{}",
        out.content
    );
    assert!(out.content.contains("2. Two"), "{}", out.content);
    assert!(!out.content.contains("Three"), "{}", out.content);
    let req = rx.recv().unwrap();
    let line = req.lines().next().unwrap();
    assert!(line.starts_with("GET /api?"), "{line}");
    assert!(line.contains("lang=en"), "{line}");
    assert!(line.contains("q=rust+lang"), "{line}");
    assert!(line.contains("n=2"), "{line}");
    let lower = req.to_ascii_lowercase();
    assert!(lower.contains("x-api-key: sekrit"), "{req}");
    assert!(lower.contains("accept: application/json"), "{req}");
}

#[tokio::test]
async fn custom_post_sends_json_body() {
    let (addr, rx) = spawn_recording("200 OK", r#"{"data":{"items":[]}}"#, 1);
    let mut custom = custom_settings(format!("http://{addr}/api"));
    custom.method = "post".into();
    custom.results_pointer = "/data/items".into();
    let out = run_custom(custom).await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("No results"), "{}", out.content);
    let req = rx.recv().unwrap();
    assert!(req.starts_with("POST /api "), "{req}");
    let body = req.split_once("\r\n\r\n").unwrap().1;
    let json: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(json, serde_json::json!({"lang": "en", "q": "q", "n": 10}));
}

#[tokio::test]
async fn custom_mapping_errors_are_clear_and_never_echo_bodies() {
    let (addr, _rx) = spawn_recording("200 OK", r#"{"hits":"secret-body"}"#, 1);
    let out = run_custom(custom_settings(format!("http://{addr}/api"))).await;
    assert!(out.is_error, "{}", out.content);
    assert!(
        out.content.contains("results_pointer `/results`"),
        "{}",
        out.content
    );
    assert!(!out.content.contains("secret-body"), "{}", out.content);

    let (addr, _rx) = spawn_recording("200 OK", r#"{"results":[{"name":"x"}]}"#, 1);
    let out = run_custom(custom_settings(format!("http://{addr}/api"))).await;
    assert!(out.is_error, "{}", out.content);
    assert!(
        out.content.contains("title_pointer `/title`"),
        "{}",
        out.content
    );

    let (addr, _rx) = spawn_recording("200 OK", "secret-body not json", 1);
    let out = run_custom(custom_settings(format!("http://{addr}/api?key=hidden"))).await;
    assert!(out.is_error, "{}", out.content);
    assert!(
        out.content.contains("Error parsing search results"),
        "{}",
        out.content
    );
    assert!(!out.content.contains("secret-body"), "{}", out.content);
    assert!(!out.content.contains("hidden"), "{}", out.content);

    // Redirects are not followed; the status is reported without the body.
    let (addr, _rx) = spawn_recording("302 Found", "secret-body", 1);
    let out = run_custom(custom_settings(format!("http://{addr}/api?key=hidden"))).await;
    assert!(out.is_error, "{}", out.content);
    assert!(out.content.contains("HTTP 302"), "{}", out.content);
    assert!(!out.content.contains("secret-body"), "{}", out.content);
    assert!(!out.content.contains("hidden"), "{}", out.content);

    let out = run_custom(custom_settings("http://127.0.0.1:1/api?key=hidden".into())).await;
    assert!(out.is_error, "{}", out.content);
    assert!(
        out.content.contains("Error performing search"),
        "{}",
        out.content
    );
    assert!(!out.content.contains("hidden"), "{}", out.content);

    let mut bad_method = custom_settings("http://127.0.0.1:1/api".into());
    bad_method.method = "PUT".into();
    let out = run_custom(bad_method).await;
    assert!(out.content.contains("method `PUT`"), "{}", out.content);

    let mut missing_env = custom_settings("http://127.0.0.1:1/api".into());
    missing_env.header_env = [(
        "X-Api-Key".to_string(),
        "WHYCODES_TEST_SEARCH_KEY_UNSET".to_string(),
    )]
    .into();
    let out = run_custom(missing_env).await;
    assert!(
        out.content.contains("WHYCODES_TEST_SEARCH_KEY_UNSET"),
        "{}",
        out.content
    );

    let err = custom_client(reqwest::Client::builder().user_agent("bad\nua").build());
    assert!(err.unwrap_err().contains("custom search client"));
}

#[tokio::test]
async fn custom_allowlist_denial_hides_the_query_string() {
    let mut ctx = crate::tool::ToolContext::unsandboxed("/");
    ctx.network = whycodes_core::NetworkPolicy {
        allowlist: vec!["example.com".into()],
        denylist: vec![],
    };
    let out = custom_tool(
        custom_settings("http://127.0.0.1:1/api?key=hidden".into()),
        &[],
    )
    .execute(serde_json::json!({"query": "q"}), &ctx)
    .await;
    assert!(out.is_error, "{}", out.content);
    assert!(
        out.content.contains("Network policy blocked host"),
        "{}",
        out.content
    );
    assert!(!out.content.contains("hidden"), "{}", out.content);
}

#[tokio::test]
async fn fallback_runs_in_order_after_failures() {
    let _env = SearchEnv::lock();
    unsafe { std::env::remove_var("SERPAPI_API_KEY") };
    let html = "<div class=\"result__snippet\">from ddg</div>\n";
    let addr = spawn_http(html, "text/html", 1);
    unsafe { std::env::set_var("WHYCODES_DDG_BASE", format!("http://{addr}")) };
    let ctx = crate::tool::ToolContext::unsandboxed("/");
    // custom fails (connect), serpapi fails (no key), ddg answers; the
    // repeated `custom` is skipped.
    let tool = custom_tool(
        custom_settings("http://127.0.0.1:1/api".into()),
        &["custom", " SerpAPI ", "duckduckgo"],
    );
    let out = tool.execute(serde_json::json!({"query": "q"}), &ctx).await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("from ddg"), "{}", out.content);

    // Every engine failing lists each one, in order.
    tool.set_settings(WebSearchSettings {
        engine: Some("bing".into()),
        fallback: vec!["serpapi".into(), "custom".into()],
        custom: None,
    });
    let out = tool.execute(serde_json::json!({"query": "q"}), &ctx).await;
    assert!(out.is_error, "{}", out.content);
    let lines: Vec<&str> = out.content.lines().collect();
    assert_eq!(lines[0], "All websearch engines failed:");
    assert!(
        lines[1].starts_with("bing: unknown websearch engine"),
        "{}",
        out.content
    );
    assert!(
        lines[2].starts_with("serpapi: SERPAPI_API_KEY"),
        "{}",
        out.content
    );
    assert!(
        lines[3].starts_with("custom: engine `custom` needs"),
        "{}",
        out.content
    );

    // No engine set: DuckDuckGo (no SerpAPI key) stays first.
    let addr = spawn_http(html, "text/html", 1);
    unsafe { std::env::set_var("WHYCODES_DDG_BASE", format!("http://{addr}")) };
    let default_tool = WebSearchTool::new();
    default_tool.set_settings(WebSearchSettings {
        fallback: vec!["custom".into()],
        ..Default::default()
    });
    let out = default_tool
        .execute(serde_json::json!({"query": "q"}), &ctx)
        .await;
    assert!(out.content.contains("from ddg"), "{}", out.content);
}

#[tokio::test]
async fn serpapi_errors_never_echo_the_api_key() {
    let _env = SearchEnv::lock();
    unsafe {
        std::env::set_var("SERPAPI_API_KEY", "hidden-key");
        std::env::set_var("WHYCODES_SERPAPI_BASE", "http://127.0.0.1:1");
    }
    let mut ctx = crate::tool::ToolContext::unsandboxed("/");
    let connect = WebSearchTool::new()
        .execute(serde_json::json!({"query": "q"}), &ctx)
        .await;
    assert!(connect.is_error, "{}", connect.content);
    assert!(
        !connect.content.contains("hidden-key"),
        "{}",
        connect.content
    );
    ctx.network = whycodes_core::NetworkPolicy {
        allowlist: vec!["example.com".into()],
        denylist: vec![],
    };
    let blocked = WebSearchTool::new()
        .execute(serde_json::json!({"query": "q"}), &ctx)
        .await;
    assert!(blocked.is_error, "{}", blocked.content);
    assert!(
        !blocked.content.contains("hidden-key"),
        "{}",
        blocked.content
    );
}

#[test]
fn settings_survive_a_poisoned_lock() {
    let tool = WebSearchTool::new();
    let shared = tool.clone();
    let joined = std::thread::spawn(move || {
        let _guard = shared.settings.lock().unwrap();
        panic!("poison websearch settings");
    })
    .join();
    assert!(joined.is_err());
    tool.set_settings(WebSearchSettings {
        engine: Some("duckduckgo".into()),
        ..Default::default()
    });
    assert_eq!(tool.settings().engine.as_deref(), Some("duckduckgo"));
}
