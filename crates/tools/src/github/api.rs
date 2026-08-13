/// Shared GitHub REST API helpers for tools (issues, PRs, etc.)
use std::env;

use reqwest::header::{HeaderMap, HeaderValue};
use whycode_core::network::NetworkPolicy;

const GITHUB_API_BASE: &str = "https://api.github.com";

/// Resolve a GitHub token: explicit argument first, then GITHUB_TOKEN env var.
pub fn resolve_token(explicit_token: Option<&str>) -> Option<String> {
    if let Some(t) = explicit_token
        && !t.is_empty()
    {
        return Some(t.to_string());
    }
    env::var("GITHUB_TOKEN").ok()
}

/// Build common headers for GitHub API requests (auth, accept, user-agent).
pub fn github_headers(token: &str) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    headers.insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|e| format!("Invalid token: {e}"))?,
    );
    headers.insert(
        "Accept",
        HeaderValue::from_static("application/vnd.github+json"),
    );
    headers.insert(
        "X-GitHub-Api-Version",
        HeaderValue::from_static("2022-11-28"),
    );
    headers.insert("User-Agent", HeaderValue::from_static("whycode"));
    Ok(headers)
}

/// Build a full GitHub API URL: https://api.github.com/{path}
pub fn api_url(path: &str) -> String {
    format!("{GITHUB_API_BASE}/{path}")
}

/// Perform a GitHub REST API request and return the body text.
pub async fn make_request(
    client: &reqwest::Client,
    method: reqwest::Method,
    path: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> Result<(reqwest::StatusCode, String), String> {
    make_request_with_policy(
        client,
        method,
        path,
        token,
        body,
        &NetworkPolicy::unrestricted(),
    )
    .await
}

/// Like [`make_request`], but enforces the session network allow/deny policy.
pub async fn make_request_with_policy(
    client: &reqwest::Client,
    method: reqwest::Method,
    path: &str,
    token: &str,
    body: Option<serde_json::Value>,
    network: &NetworkPolicy,
) -> Result<(reqwest::StatusCode, String), String> {
    let headers = github_headers(token)?;
    let url = api_url(path);
    network.check_url(&url)?;

    let mut req = client.request(method, &url).headers(headers);
    if let Some(b) = body {
        req = req.json(&b);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("GitHub API request failed: {e}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("Failed to read GitHub API response: {e}"))?;

    Ok((status, text))
}
