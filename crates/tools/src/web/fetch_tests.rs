use super::*;
use whycodes_core::NetworkPolicy;

#[tokio::test]
async fn network_allowlist_blocks_disallowed_host() {
    let tool = WebFetchTool::new();
    let mut ctx = ToolContext::unsandboxed("/tmp");
    ctx.network = NetworkPolicy {
        allowlist: vec!["allowed.example".into()],
        denylist: vec![],
    };
    let result = tool
        .execute(json!({ "url": "https://evil.example/secret" }), &ctx)
        .await;
    assert!(result.is_error);
    assert!(
        result.content.contains("Network policy blocked")
            || result.content.contains("blocked host"),
        "unexpected: {}",
        result.content
    );
}

#[tokio::test]
async fn execute_requires_url_and_fetches_loopback() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    let ctx = ToolContext::unsandboxed("/");
    let missing = WebFetchTool::new().execute(json!({}), &ctx).await;
    assert!(missing.is_error, "{}", missing.content);
    assert!(
        missing.content.contains("URL is required"),
        "{}",
        missing.content
    );

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let body = "hello-fetch";
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
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
    let url = format!("http://{addr}/page");
    let out = WebFetchTool::new()
        .execute(json!({ "url": url }), &ctx)
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("hello-fetch"), "{}", out.content);
    assert!(out.content.contains("200"), "{}", out.content);
}

#[test]
fn json_pretty_printed() {
    let out = format_body("application/json", r#"{"version":"4.5.1","name":"nuxt"}"#);
    assert!(out.contains("\n"));
    assert!(out.contains("\"version\": \"4.5.1\"") || out.contains("\"version\":\"4.5.1\""));
    assert!(out.contains("4.5.1"));
}

#[test]
fn json_sniffed_without_content_type() {
    let out = format_body("", r#"{"version":"1.0.0"}"#);
    assert!(out.contains("1.0.0"));
    assert!(!out.contains("<"));
}

#[test]
fn plain_text_passthrough() {
    let out = format_body("text/plain", "hello\n\nworld");
    assert!(out.contains("hello"));
    assert!(out.contains("world"));
}

#[test]
fn html_strips_tags_and_scripts() {
    let html = r#"<html><head><script>alert(1)</script><style>.x{}</style></head>
        <body><h1>Title</h1><p>Hello &amp; <b>world</b></p></body></html>"#;
    let out = html_to_text(html);
    assert!(!out.contains("alert"));
    assert!(!out.contains(".x{}"));
    assert!(out.contains("Title"));
    assert!(
        out.contains("Hello & world") || out.contains("Hello &amp; world") || out.contains("Hello")
    );
    assert!(out.contains("world"));
    assert!(!out.contains("<h1>"));
}

#[test]
fn truncate_respects_char_boundary() {
    let s = "héllo world";
    let out = truncate_chars(s, 3);
    assert!(out.contains("[truncated]"));
    // Must not panic on multi-byte.
}

#[test]
fn remaining_format_and_html_helpers() {
    let t = WebFetchTool;
    assert_eq!(t.name(), "webfetch");
    assert!(!t.description().is_empty());
    let _ = t.parameters();
    assert!(looks_like_json("[1]"));
    assert!(!looks_like_json("nope"));
    assert!(looks_like_html("<!DOCTYPE html>"));
    assert!(looks_like_html("<p></p>"));
    assert!(!looks_like_html("plain"));
    let md = format_body("text/markdown", "hello\n\nworld");
    assert!(md.contains("hello"));
    let html = format_body("text/html", "<p>Hi</p>");
    assert!(html.contains("Hi"));
    let unknown = format_body("application/octet-stream", "plain text");
    assert!(unknown.contains("plain"));
    let markup = format_body("", "<div></div>");
    assert!(!markup.contains("<div"));
    let bad_json = format_body("application/json", "{not json");
    assert_eq!(bad_json, "{not json");
    let decoded = decode_basic_entities("&lt;&gt;&quot;&#39;&apos;&nbsp;");
    assert!(decoded.contains('<'));
    let short = truncate_chars("abc", 10);
    assert_eq!(short, "abc");
    let _ = http_client();
    let _ = http_client();
}

#[tokio::test]
async fn fetch_connect_error_and_html_fallback() {
    let ctx = ToolContext::unsandboxed("/");
    let err = WebFetchTool::new()
        .execute(json!({ "url": "http://127.0.0.1:1/missing" }), &ctx)
        .await;
    assert!(err.is_error, "{}", err.content);
    assert!(
        err.content.contains("Error fetching URL"),
        "{}",
        err.content
    );

    let htmlish = format_body("application/octet-stream", "<div>Hi</div>");
    assert!(htmlish.contains("Hi"));
    let truncated = truncate_chars("héllo", 2);
    assert!(truncated.contains("[truncated]"));
    let unclosed = strip_tag_blocks("<script>alert(1)", &["script"]);
    assert!(unclosed.contains("alert") || unclosed.contains("<script"));
}
