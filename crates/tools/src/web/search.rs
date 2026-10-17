use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::json;

use super::fetch::{html_to_text, http_client};
use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

/// Runtime `[websearch]` settings: engine order and the custom endpoint.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebSearchSettings {
    /// `serpapi` / `duckduckgo` / `custom`. `None` keeps the default choice
    /// (SerpAPI when `SERPAPI_API_KEY` is set, else DuckDuckGo).
    pub engine: Option<String>,
    /// Engines tried in order when the primary returns an error.
    pub fallback: Vec<String>,
    /// The `custom` engine's endpoint and JSON Pointer mapping.
    pub custom: Option<CustomSearchSettings>,
}

/// A JSON search API mapped onto results with JSON Pointers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CustomSearchSettings {
    pub endpoint: String,
    /// `GET` sends a query string; `POST` sends the parameters as a JSON body.
    pub method: String,
    pub query_param: String,
    pub count_param: Option<String>,
    pub params: BTreeMap<String, String>,
    pub headers: BTreeMap<String, String>,
    /// Header name → environment variable holding its value.
    pub header_env: BTreeMap<String, String>,
    pub results_pointer: String,
    pub title_pointer: String,
    pub url_pointer: String,
    pub snippet_pointer: String,
}

/// Timeout for one custom-endpoint request.
const CUSTOM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

#[derive(Clone, Default)]
pub struct WebSearchTool {
    settings: Arc<Mutex<Arc<WebSearchSettings>>>,
}

impl WebSearchTool {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the engine settings. Clones of this tool share them.
    pub fn set_settings(&self, settings: WebSearchSettings) {
        *lock_settings(&self.settings) = Arc::new(settings);
    }

    fn settings(&self) -> Arc<WebSearchSettings> {
        Arc::clone(&lock_settings(&self.settings))
    }
}

fn lock_settings(m: &Mutex<Arc<WebSearchSettings>>) -> MutexGuard<'_, Arc<WebSearchSettings>> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "websearch"
    }

    fn description(&self) -> &str {
        "Search the web and return result snippets. The engine comes from `[websearch]` config; \
         by default SerpAPI when SERPAPI_API_KEY is set, else DuckDuckGo HTML. \
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

            let settings = self.settings();
            let chain = engine_chain(&settings);
            let mut failures: Vec<(&str, String)> = Vec::new();
            for engine in &chain {
                let out = run_engine(ctx, &settings, engine, query, num_results).await;
                if !out.is_error {
                    return out;
                }
                failures.push((engine, out.content));
            }
            search_failed(&failures)
        })
    }
}

/// Primary engine then each fallback, without repeats.
fn engine_chain(settings: &WebSearchSettings) -> Vec<String> {
    let primary = match settings.engine.as_deref() {
        Some(name) => name.trim().to_ascii_lowercase(),
        None => default_engine(),
    };
    let mut chain = vec![primary];
    for name in &settings.fallback {
        let name = name.trim().to_ascii_lowercase();
        if !chain.contains(&name) {
            chain.push(name);
        }
    }
    chain
}

fn default_engine() -> String {
    match std::env::var("SERPAPI_API_KEY") {
        Ok(_) => "serpapi".to_string(),
        Err(err) => {
            note_no_serpapi(&err);
            "duckduckgo".to_string()
        }
    }
}

async fn run_engine(
    ctx: &ToolContext,
    settings: &WebSearchSettings,
    engine: &str,
    query: &str,
    num_results: u64,
) -> ToolResult {
    match engine {
        "serpapi" => match std::env::var("SERPAPI_API_KEY") {
            Ok(api_key) => serpapi_search(ctx, query, num_results, &api_key).await,
            Err(err) => search_blocked(&format!("SERPAPI_API_KEY is unavailable ({err}).")),
        },
        "duckduckgo" => ddg_search(ctx, query, num_results).await,
        "custom" => match &settings.custom {
            Some(custom) => custom_search(ctx, custom, query, num_results).await,
            None => search_blocked(
                "engine `custom` needs a `[websearch.custom]` table with an `endpoint`.",
            ),
        },
        other => search_blocked(&format!(
            "unknown websearch engine `{other}` (expected serpapi, duckduckgo or custom)."
        )),
    }
}

/// One engine's error as-is; several engines' errors one per line.
fn search_failed(failures: &[(&str, String)]) -> ToolResult {
    let content = match failures {
        [(_, only)] => only.clone(),
        _ => {
            let lines: Vec<String> = failures.iter().map(|(e, m)| format!("{e}: {m}")).collect();
            format!("All websearch engines failed:\n{}", lines.join("\n"))
        }
    };
    search_blocked(&content)
}

async fn custom_search(
    ctx: &ToolContext,
    custom: &CustomSearchSettings,
    query: &str,
    num_results: u64,
) -> ToolResult {
    // The policy message echoes the URL it checked: never hand it a query string.
    let bare = endpoint_without_query(&custom.endpoint);
    match ctx.network.check_url(bare) {
        Ok(()) => note_search_url_ok(),
        Err(msg) => return search_blocked(&msg),
    }
    let request = match custom_request(custom, query, num_results) {
        Ok(request) => request,
        Err(msg) => return search_blocked(&msg),
    };
    let response = match request.send().await {
        Ok(response) => response,
        Err(e) => return search_request_error(&e.without_url().to_string()),
    };
    let status = response.status();
    if !status.is_success() {
        return search_blocked(&format!(
            "Custom search endpoint `{bare}` returned HTTP {}.",
            status.as_u16()
        ));
    }
    match response.json::<serde_json::Value>().await {
        Ok(data) => custom_results(custom, &data, num_results as usize),
        Err(e) => search_parse_error(&e.without_url().to_string()),
    }
}

fn endpoint_without_query(endpoint: &str) -> &str {
    endpoint.split(['?', '#']).next().unwrap_or(endpoint)
}

fn custom_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .user_agent(concat!("whycodes-websearch/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(CUSTOM_TIMEOUT)
}

/// No fallback to a default client: it would follow redirects.
fn custom_client(
    built: Result<reqwest::Client, reqwest::Error>,
) -> Result<reqwest::Client, String> {
    built.map_err(|e| format!("Could not build the custom search client: {e}"))
}

fn custom_request(
    custom: &CustomSearchSettings,
    query: &str,
    num_results: u64,
) -> Result<reqwest::RequestBuilder, String> {
    let client = custom_client(custom_client_builder().build())?;
    let post = match custom.method.trim().to_ascii_uppercase().as_str() {
        "GET" => false,
        "POST" => true,
        other => {
            return Err(format!(
                "websearch.custom.method `{other}` is not supported (use GET or POST)."
            ));
        }
    };
    let mut request = if post {
        let mut body = serde_json::Map::new();
        for (k, v) in &custom.params {
            body.insert(k.clone(), json!(v));
        }
        body.insert(custom.query_param.clone(), json!(query));
        if let Some(count) = &custom.count_param {
            body.insert(count.clone(), json!(num_results));
        }
        client.post(&custom.endpoint).json(&body)
    } else {
        let mut pairs: Vec<(String, String)> = custom.params.clone().into_iter().collect();
        pairs.push((custom.query_param.clone(), query.to_string()));
        if let Some(count) = &custom.count_param {
            pairs.push((count.clone(), num_results.to_string()));
        }
        client.get(&custom.endpoint).query(&pairs)
    };
    for (name, value) in &custom.headers {
        request = request.header(name, value);
    }
    for (name, var) in &custom.header_env {
        match std::env::var(var) {
            Ok(value) => request = request.header(name, value),
            Err(err) => {
                return Err(format!(
                    "Header `{name}` reads env var `{var}`, which is unavailable ({err})."
                ));
            }
        }
    }
    Ok(request)
}

fn custom_results(
    custom: &CustomSearchSettings,
    data: &serde_json::Value,
    num_results: usize,
) -> ToolResult {
    let Some(items) = data
        .pointer(&custom.results_pointer)
        .and_then(|v| v.as_array())
    else {
        return search_blocked(&format!(
            "results_pointer `{}` does not point at an array in the response.",
            custom.results_pointer
        ));
    };
    let mapped: Vec<serde_json::Value> = items
        .iter()
        .filter_map(|item| {
            let title = item.pointer(&custom.title_pointer)?.as_str()?;
            let link = item.pointer(&custom.url_pointer)?.as_str()?;
            let snippet = item
                .pointer(&custom.snippet_pointer)
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Some(json!({"title": title, "link": link, "snippet": snippet}))
        })
        .take(num_results)
        .collect();
    if mapped.is_empty() && !items.is_empty() {
        return search_blocked(&format!(
            "No result has a string at title_pointer `{}` and url_pointer `{}`.",
            custom.title_pointer, custom.url_pointer
        ));
    }
    organic_results(&json!({ "organic_results": mapped }))
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
    // The URL carries the API key: keep it out of policy and error messages.
    match ctx.network.check_url(endpoint_without_query(&url)) {
        Ok(()) => note_search_url_ok(),
        Err(msg) => return search_blocked(&msg),
    }
    match http_client().get(&url).send().await {
        Ok(response) => match response.json::<serde_json::Value>().await {
            Ok(data) => organic_results(&data),
            Err(e) => search_parse_error(&e.without_url().to_string()),
        },
        Err(e) => search_request_error(&e.without_url().to_string()),
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
