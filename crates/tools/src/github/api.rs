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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_token_takes_precedence_without_environment_access() {
        assert_eq!(
            resolve_token(Some("explicit")),
            Some("explicit".to_string())
        );
    }

    #[test]
    fn headers_include_github_requirements() {
        let headers = github_headers("secret").expect("valid token should build headers");

        assert_eq!(headers["Authorization"], "Bearer secret");
        assert_eq!(headers["Accept"], "application/vnd.github+json");
        assert_eq!(headers["X-GitHub-Api-Version"], "2022-11-28");
        assert_eq!(headers["User-Agent"], "whycode");
    }

    #[test]
    fn invalid_header_token_is_rejected() {
        let error = github_headers("bad\ntoken").expect_err("newline must be rejected");
        assert!(error.starts_with("Invalid token:"));
    }

    #[test]
    fn api_url_preserves_the_requested_path() {
        assert_eq!(
            api_url("repos/whycode/whycode/issues?state=open"),
            "https://api.github.com/repos/whycode/whycode/issues?state=open"
        );
    }

    #[tokio::test]
    async fn policy_rejection_happens_before_network_io() {
        let policy = NetworkPolicy {
            allowlist: vec!["example.com".to_string()],
            denylist: Vec::new(),
        };
        let error = make_request_with_policy(
            &reqwest::Client::new(),
            reqwest::Method::GET,
            "repos/owner/repo",
            "token",
            None,
            &policy,
        )
        .await
        .expect_err("GitHub should be blocked by policy");

        assert!(error.contains("Network policy blocked host `api.github.com`"));
    }
}
