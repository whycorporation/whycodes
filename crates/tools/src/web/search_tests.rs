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
    let t = WebSearchTool;
    assert_eq!(t.name(), "websearch");
    assert_eq!(WebSearchTool.name(), "websearch");
    assert!(!t.description().is_empty());
    assert_eq!(t.parameters()["required"][0], "query");
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
