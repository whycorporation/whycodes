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
            match std::env::var("SERPAPI_API_KEY") {
                Ok(api_key) => return serpapi_search(ctx, query, num_results, &api_key).await,
                Err(err) => note_no_serpapi(&err),
            }

            ddg_search(ctx, query, num_results).await
        })
    }
}

fn note_no_serpapi(_err: &std::env::VarError) {}

async fn serpapi_search(
    ctx: &ToolContext,
    query: &str,
    num_results: u64,
    api_key: &str,
) -> ToolResult {
    let url = format!(
        "{}/search?q={}&api_key={}&num={}&engine=google",
        search_host("WHYCODES_SERPAPI_BASE", "https://serpapi.com"),
        urlencoding(query),
        api_key,
        num_results
    );
    match ctx.network.check_url(&url) {
        Ok(()) => note_search_url_ok(),
        Err(msg) => return search_blocked(&msg),
    }
    match http_client().get(&url).send().await {
        Ok(response) => match response.json::<serde_json::Value>().await {
            Ok(data) => organic_results(&data),
            Err(e) => search_parse_error(&e.to_string()),
        },
        Err(e) => search_request_error(&e.to_string()),
    }
}

fn note_search_url_ok() {}

fn search_blocked(msg: &str) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: msg.to_string(),
        is_error: true,
    }
}

fn organic_results(data: &serde_json::Value) -> ToolResult {
    let mut results = String::new();
    match data["organic_results"].as_array() {
        Some(organic) => append_organic(&mut results, organic),
        None => note_no_organic(),
    }
    ToolResult {
        tool_call_id: String::new(),
        content: if results.is_empty() {
            "No results found.".to_string()
        } else {
            results
        },
        is_error: false,
    }
}

fn note_no_organic() {}

fn append_organic(results: &mut String, organic: &[serde_json::Value]) {
    for (i, result) in organic.iter().enumerate() {
        let title = strip_markup(result["title"].as_str().unwrap_or("No title"));
        let link = result["link"].as_str().unwrap_or("No link");
        let snippet = strip_markup(result["snippet"].as_str().unwrap_or(""));
        results.push_str(&format!(
            "{}. {}\n   {}\n   {}\n\n",
            i + 1,
            title,
            link,
            snippet
        ));
    }
}

fn search_parse_error(e: &str) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: format!("Error parsing search results: {e}"),
        is_error: true,
    }
}

fn search_request_error(e: &str) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: format!("Error performing search: {e}"),
        is_error: true,
    }
}

async fn ddg_search(ctx: &ToolContext, query: &str, num_results: u64) -> ToolResult {
    let url = format!(
        "{}/html/?q={}",
        search_host("WHYCODES_DDG_BASE", "https://html.duckduckgo.com"),
        urlencoding(query)
    );
    match ctx.network.check_url(&url) {
        Ok(()) => note_search_url_ok(),
        Err(msg) => return search_blocked(&msg),
    }
    match http_client().get(&url).send().await {
        Ok(response) => search_html_result(
            response.text().await.map_err(search_body_error),
            num_results as usize,
        ),
        Err(e) => search_request_error(&e.to_string()),
    }
}

/// Strip residual HTML tags/entities from SERP snippets.
fn search_body_error(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn search_text_failed(e: &str) -> ToolResult {
    search_read_error(e)
}

fn search_html_result(result: Result<String, String>, num_results: usize) -> ToolResult {
    match result {
        Ok(html) => {
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
            results.truncate(num_results);
            ToolResult {
                tool_call_id: String::new(),
                content: if results.is_empty() {
                    "No results found. Set SERPAPI_API_KEY for better results.".to_string()
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
        Err(e) => search_text_failed(&e),
    }
}

fn search_read_error(e: &str) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: format!("Error reading response: {e}"),
        is_error: true,
    }
}

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
    search_host_with(env_key, default, cfg!(test))
}

fn search_host_with(env_key: &str, default: &str, use_test: bool) -> String {
    if use_test
        && let Ok(base) = std::env::var(env_key)
        && !base.is_empty()
    {
        return base;
    }
    let _ = env_key;
    default.to_string()
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
