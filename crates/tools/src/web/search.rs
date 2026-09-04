use serde_json::json;

use super::fetch::{html_to_text, http_client};
use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

pub struct WebSearchTool;

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WebSearchTool {
    pub fn new() -> Self {
        Self
    }
}
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "websearch"
    }

    fn description(&self) -> &str {
        "Search the web and return result snippets. Requires SERPAPI_API_KEY for best results \
         (falls back to DuckDuckGo HTML). \
         For 'latest version' questions: do not pin the query to a past calendar year \
         (e.g. avoid '2024'/'2025'); search 'Nuxt latest release' or fetch canonical APIs \
         via webfetch (npm registry, GitHub Releases, official docs)."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query. Prefer current phrasing without a past year for 'latest' lookups."
                },
                "num_results": {
                    "type": "integer",
                    "description": "Number of results to return (default: 10)"
                }
            },
            "required": ["query"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            let query = args["query"].as_str().unwrap_or("");
            let num_results = args["num_results"].as_u64().unwrap_or(10);

            if query.is_empty() {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: "Query is required.".to_string(),
                    is_error: true,
                };
            }

            // Try SerpAPI first
            if let Ok(api_key) = std::env::var("SERPAPI_API_KEY") {
                let url = format!(
                    "{}/search?q={}&api_key={}&num={}&engine=google",
                    search_host("WHYCODES_SERPAPI_BASE", "https://serpapi.com"),
                    urlencoding(query),
                    api_key,
                    num_results
                );
                if let Err(msg) = ctx.network.check_url(&url) {
                    return ToolResult {
                        tool_call_id: String::new(),
                        content: msg,
                        is_error: true,
                    };
                }

                match http_client().get(&url).send().await {
                    Ok(response) => match response.json::<serde_json::Value>().await {
                        Ok(data) => {
                            let mut results = String::new();
                            if let Some(organic) = data["organic_results"].as_array() {
                                for (i, result) in organic.iter().enumerate() {
                                    let title = strip_markup(
                                        result["title"].as_str().unwrap_or("No title"),
                                    );
                                    let link = result["link"].as_str().unwrap_or("No link");
                                    let snippet =
                                        strip_markup(result["snippet"].as_str().unwrap_or(""));
                                    results.push_str(&format!(
                                        "{}. {}\n   {}\n   {}\n\n",
                                        i + 1,
                                        title,
                                        link,
                                        snippet
                                    ));
                                }
                            }

                            return ToolResult {
                                tool_call_id: String::new(),
                                content: if results.is_empty() {
                                    "No results found.".to_string()
                                } else {
                                    results
                                },
                                is_error: false,
                            };
                        }
                        Err(e) => {
                            return ToolResult {
                                tool_call_id: String::new(),
                                content: format!("Error parsing search results: {}", e),
                                is_error: true,
                            };
                        }
                    },
                    Err(e) => {
                        return ToolResult {
                            tool_call_id: String::new(),
                            content: format!("Error performing search: {}", e),
                            is_error: true,
                        };
                    }
                }
            }

            // Fallback: try DuckDuckGo HTML search
            let url = format!(
                "{}/html/?q={}",
                search_host("WHYCODES_DDG_BASE", "https://html.duckduckgo.com"),
                urlencoding(query)
            );

            if let Err(msg) = ctx.network.check_url(&url) {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: msg,
                    is_error: true,
                };
            }

            match http_client().get(&url).send().await {
                Ok(response) => match response.text().await {
                    Ok(html) => {
                        // Simple extraction of result snippets
                        let mut results: Vec<String> = Vec::new();
                        for line in html.lines() {
                            if line.contains("result__snippet")
                                && let Some(start) = line.find('>')
                                && let Some(end) = line.rfind('<')
                                && start + 1 < end
                            {
                                let snippet = strip_markup(&line[start + 1..end]);
                                if !snippet.is_empty() {
                                    results.push(snippet);
                                }
                            }
                        }

                        results.truncate(num_results as usize);

                        ToolResult {
                            tool_call_id: String::new(),
                            content: if results.is_empty() {
                                "No results found. Set SERPAPI_API_KEY for better results."
                                    .to_string()
                            } else {
                                results
                                    .iter()
                                    .enumerate()
                                    .map(|(i, s)| format!("{}. {}", i + 1, s))
                                    .collect::<Vec<_>>()
                                    .join("\n\n")
                            },
                            is_error: false,
                        }
                    }
                    Err(e) => ToolResult {
                        tool_call_id: String::new(),
                        content: format!("Error reading response: {}", e),
                        is_error: true,
                    },
                },
                Err(e) => ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Error performing search: {}", e),
                    is_error: true,
                },
            }
        })
    }
}

/// Strip residual HTML tags/entities from SERP snippets.
fn strip_markup(s: &str) -> String {
    if s.contains('<') || s.contains('&') {
        html_to_text(s)
    } else {
        s.trim().to_string()
    }
}

fn urlencoding(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            ' ' => "+".to_string(),
            _ => format!("%{:02X}", c as u8),
        })
        .collect()
}

fn search_host(env_key: &str, default: &str) -> String {
    #[cfg(test)]
    if let Ok(base) = std::env::var(env_key)
        && !base.is_empty()
    {
        return base;
    }
    let _ = env_key;
    default.to_string()
}

#[cfg(test)]
mod tests {
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
}
