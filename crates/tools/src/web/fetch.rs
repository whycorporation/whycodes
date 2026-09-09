use std::sync::OnceLock;

use serde_json::json;

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

/// Process-wide client so TLS/connection pool stays warm across fetches.
pub(crate) fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(concat!("whycodes-webfetch/", env!("CARGO_PKG_VERSION")))
            .pool_max_idle_per_host(4)
            .tcp_nodelay(true)
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

pub struct WebFetchTool;

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WebFetchTool {
    pub fn new() -> Self {
        Self
    }
}
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "webfetch"
    }

    fn description(&self) -> &str {
        "Fetch content from a URL and return readable text. \
         Prefers machine-readable responses (JSON, Markdown, plain text) via Accept headers. \
         For package versions prefer registry APIs (e.g. https://registry.npmjs.org/<pkg>/latest) \
         or GitHub Releases over marketing HTML pages. \
         If a site exposes llm.txt / llms.txt, fetch that URL when you need LLM-oriented docs."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch"
                },
                "max_length": {
                    "type": "integer",
                    "description": "Maximum content length to return (default: 8000)"
                }
            },
            "required": ["url"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            let url = args["url"].as_str().unwrap_or("");
            let max_length = args["max_length"].as_u64().unwrap_or(8000) as usize;

            if url.is_empty() {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: "URL is required.".to_string(),
                    is_error: true,
                };
            }

            if let Err(msg) = ctx.network.check_url(url) {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: msg,
                    is_error: true,
                };
            }

            let request = http_client().get(url).header(
            reqwest::header::ACCEPT,
            "application/json, text/markdown;q=0.9, text/plain;q=0.8, text/html;q=0.5, */*;q=0.1",
        );

            match request.send().await {
                Ok(response) => {
                    let status = response.status();
                    let content_type = response
                        .headers()
                        .get(reqwest::header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    fetch_bytes_result(
                        url,
                        status.as_u16(),
                        !status.is_success(),
                        &content_type,
                        max_length,
                        response
                            .bytes()
                            .await
                            .map(|b| b.to_vec())
                            .map_err(|e| e.to_string()),
                    )
                }
                Err(e) => ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Error fetching URL: {e}"),
                    is_error: true,
                },
            }
        })
    }
}

fn fetch_bytes_failed(e: &str) -> ToolResult {
    fetch_read_error(e)
}

fn fetch_bytes_result(
    url: &str,
    status: u16,
    is_error: bool,
    content_type: &str,
    max_length: usize,
    result: Result<Vec<u8>, String>,
) -> ToolResult {
    match result {
        Ok(raw) => {
            let raw = String::from_utf8_lossy(&raw);
            let body = format_body(content_type, &raw);
            let truncated = truncate_chars(&body, max_length);
            ToolResult {
                tool_call_id: String::new(),
                content: format!(
                    "URL: {url}\nStatus: {status}\nContent-Type: {content_type}\n\n{truncated}"
                ),
                is_error,
            }
        }
        Err(e) => fetch_bytes_failed(&e),
    }
}

fn fetch_read_error(e: &str) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: format!("Error reading response: {e}"),
        is_error: true,
    }
}

/// Format response body based on Content-Type (and a light sniff of the payload).
fn format_body(content_type: &str, raw: &str) -> String {
    let ct = content_type.to_ascii_lowercase();
    let trimmed = raw.trim_start();

    // JSON: pretty-print when possible; never HTML-strip.
    if ct.contains("json") || looks_like_json(trimmed) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
            return serde_json::to_string_pretty(&value).unwrap_or_else(|_| raw.to_string());
        }
        return raw.to_string();
    }

    // Markdown / plain: return as-is (collapse only extreme blank runs later if needed).
    if ct.contains("markdown") || ct.contains("text/plain") || ct.starts_with("text/x-markdown") {
        return normalize_whitespace(raw);
    }

    // HTML (or unknown that looks like HTML): strip tags carefully.
    if ct.contains("html") || looks_like_html(trimmed) {
        return html_to_text(raw);
    }

    // Default: if it still looks like markup, strip; else plain.
    fallback_body(raw, looks_like_html(trimmed))
}

fn fallback_body(raw: &str, html: bool) -> String {
    if html {
        html_to_text(raw)
    } else {
        normalize_whitespace(raw)
    }
}

fn looks_like_json(s: &str) -> bool {
    (s.starts_with('{') && s.contains('}')) || (s.starts_with('[') && s.contains(']'))
}

fn looks_like_html(s: &str) -> bool {
    let lower = s.get(..200).unwrap_or(s).to_ascii_lowercase();
    lower.contains("<!doctype html")
        || lower.contains("<html")
        || lower.contains("<head")
        || lower.contains("<body")
        || (lower.contains('<') && lower.contains("</"))
}

/// Strip scripts/styles/tags and decode a small set of HTML entities.
pub fn html_to_text(html: &str) -> String {
    // Drop script/style blocks first (case-insensitive, non-greedy-ish via linear scan).
    let without_blocks = strip_tag_blocks(html, &["script", "style", "noscript"]);
    let mut text = String::with_capacity(without_blocks.len());
    let mut in_tag = false;
    for c in without_blocks.chars() {
        if c == '<' {
            in_tag = true;
            continue;
        }
        if c == '>' {
            in_tag = false;
            // Treat tag boundaries as soft whitespace so words don't glue.
            text.push(' ');
            continue;
        }
        if !in_tag {
            text.push(c);
        }
    }
    normalize_whitespace(&decode_basic_entities(&text))
}

fn strip_tag_blocks(html: &str, tags: &[&str]) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    let len = html.len();
    while i < len {
        let mut skipped = false;
        for tag in tags {
            let open = format!("<{tag}");
            if lower[i..].starts_with(&open)
                && let Some(rel) = lower[i..].find('>')
            {
                let after_open = i + rel + 1;
                let close = format!("</{tag}>");
                if let Some(rel_close) = lower[after_open..].find(&close) {
                    i = after_open + rel_close + close.len();
                    skipped = true;
                    break;
                }
            }
        }
        if skipped {
            skip_closed_tag();
            continue;
        }
        let ch = next_html_char(&html[i..]).unwrap_or_else(html_exhausted);
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn skip_closed_tag() {}

fn next_html_char(rest: &str) -> Option<char> {
    rest.chars().next()
}

fn html_exhausted() -> char {
    '\0'
}

fn decode_basic_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

fn normalize_whitespace(s: &str) -> String {
    let lines: Vec<String> = s
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect();
    lines.join("\n")
}

fn truncate_chars(s: &str, max_length: usize) -> String {
    if s.len() <= max_length {
        return s.to_string();
    }
    // Prefer a char boundary.
    let mut end = max_length.min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...\n[truncated]", &s[..end])
}

#[cfg(test)]
#[path = "fetch_tests.rs"]
mod tests;
