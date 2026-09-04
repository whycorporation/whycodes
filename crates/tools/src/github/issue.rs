use serde_json::json;

use super::api::{api_url, github_headers, resolve_token};
use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

pub struct GithubIssueTool;

impl Default for GithubIssueTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GithubIssueTool {
    pub fn new() -> Self {
        Self
    }
}
impl Tool for GithubIssueTool {
    fn name(&self) -> &str {
        "github_issue"
    }

    fn description(&self) -> &str {
        "Manage GitHub issues: create, list, view, close, reopen, or comment on issues."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "list", "view", "close", "reopen", "comment"],
                    "description": "Action to perform on GitHub issues"
                },
                "owner": {
                    "type": "string",
                    "description": "Repository owner (username or organization)"
                },
                "repo": {
                    "type": "string",
                    "description": "Repository name"
                },
                "token": {
                    "type": "string",
                    "description": "GitHub personal access token (defaults to GITHUB_TOKEN env var)"
                },
                "title": {
                    "type": "string",
                    "description": "Issue title (required for 'create')"
                },
                "body": {
                    "type": "string",
                    "description": "Issue body or comment body"
                },
                "labels": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Labels to apply (for 'create' action)"
                },
                "issue_number": {
                    "type": "integer",
                    "description": "Issue number (required for view, close, reopen, comment)"
                },
                "state": {
                    "type": "string",
                    "enum": ["open", "closed", "all"],
                    "description": "Filter by state (for 'list' action, default: open)"
                },
                "per_page": {
                    "type": "integer",
                    "description": "Results per page (for 'list' action, default: 30, max: 100)"
                }
            },
            "required": ["action", "owner", "repo"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            let action = args["action"].as_str().unwrap_or("");
            let owner = args["owner"].as_str().unwrap_or("");
            let repo = args["repo"].as_str().unwrap_or("");

            if owner.is_empty() || repo.is_empty() {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: "owner and repo are required.".to_string(),
                    is_error: true,
                };
            }

            if let Err(msg) = ctx.network.check_url("https://api.github.com/") {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: msg,
                    is_error: true,
                };
            }

            let token = match resolve_token(args["token"].as_str()) {
                Some(t) => t,
                None => {
                    return ToolResult {
                        tool_call_id: String::new(),
                        content:
                            "GitHub token not found. Set GITHUB_TOKEN env var or pass 'token' arg."
                                .to_string(),
                        is_error: true,
                    };
                }
            };

            let headers = match github_headers(&token) {
                Ok(h) => h,
                Err(e) => {
                    return ToolResult {
                        tool_call_id: String::new(),
                        content: format!("Failed to build headers: {e}"),
                        is_error: true,
                    };
                }
            };

            let client = reqwest::Client::new();

            let result = match action {
                "create" => issue_create(&client, &headers, owner, repo, &args).await,
                "list" => issue_list(&client, &headers, owner, repo, &args).await,
                "view" => issue_view(&client, &headers, owner, repo, &args).await,
                "close" => issue_set_state(&client, &headers, owner, repo, &args, "closed").await,
                "reopen" => issue_set_state(&client, &headers, owner, repo, &args, "open").await,
                "comment" => issue_comment(&client, &headers, owner, repo, &args).await,
                other => {
                    return ToolResult {
                        tool_call_id: String::new(),
                        content: format!(
                            "Unknown action: '{other}'. Valid: create, list, view, close, reopen, comment"
                        ),
                        is_error: true,
                    };
                }
            };

            match result {
                Ok(body) => ToolResult {
                    tool_call_id: String::new(),
                    content: body,
                    is_error: false,
                },
                Err(e) => ToolResult {
                    tool_call_id: String::new(),
                    content: e,
                    is_error: true,
                },
            }
        })
    }
}

async fn request(
    client: &reqwest::Client,
    headers: &reqwest::header::HeaderMap,
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<String, String> {
    let url = api_url(path);
    let mut req = client.request(method, &url).headers(headers.clone());
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
        .map_err(|e| format!("Failed to read response: {e}"))?;

    if status.is_success() {
        Ok(text)
    } else {
        Err(format!(
            "GitHub API error ({} {status}): {}",
            status.as_u16(),
            text
        ))
    }
}

async fn issue_create(
    client: &reqwest::Client,
    headers: &reqwest::header::HeaderMap,
    owner: &str,
    repo: &str,
    args: &serde_json::Value,
) -> Result<String, String> {
    let title = args["title"].as_str().unwrap_or("");
    if title.is_empty() {
        return Err("title is required for create action.".to_string());
    }

    let mut body = json!({ "title": title });
    if let Some(b) = args["body"].as_str()
        && !b.is_empty()
    {
        body["body"] = json!(b);
    }
    if let Some(labels) = args["labels"].as_array() {
        body["labels"] = json!(labels);
    }

    let path = format!("repos/{owner}/{repo}/issues");
    request(client, headers, reqwest::Method::POST, &path, Some(body)).await
}

async fn issue_list(
    client: &reqwest::Client,
    headers: &reqwest::header::HeaderMap,
    owner: &str,
    repo: &str,
    args: &serde_json::Value,
) -> Result<String, String> {
    let state = args["state"].as_str().unwrap_or("open");
    let per_page = args["per_page"].as_u64().unwrap_or(30).min(100);
    let path = format!("repos/{owner}/{repo}/issues?state={state}&per_page={per_page}&filter=all");
    request(client, headers, reqwest::Method::GET, &path, None).await
}

async fn issue_view(
    client: &reqwest::Client,
    headers: &reqwest::header::HeaderMap,
    owner: &str,
    repo: &str,
    args: &serde_json::Value,
) -> Result<String, String> {
    let number = args["issue_number"]
        .as_u64()
        .ok_or("issue_number is required and must be an integer for view action.")?;
    let path = format!("repos/{owner}/{repo}/issues/{number}");
    request(client, headers, reqwest::Method::GET, &path, None).await
}

async fn issue_set_state(
    client: &reqwest::Client,
    headers: &reqwest::header::HeaderMap,
    owner: &str,
    repo: &str,
    args: &serde_json::Value,
    state: &str,
) -> Result<String, String> {
    let number = args["issue_number"].as_u64().ok_or(format!(
        "issue_number is required and must be an integer for {state} action."
    ))?;
    let path = format!("repos/{owner}/{repo}/issues/{number}");
    let body = json!({ "state": state });
    request(client, headers, reqwest::Method::PATCH, &path, Some(body)).await
}

async fn issue_comment(
    client: &reqwest::Client,
    headers: &reqwest::header::HeaderMap,
    owner: &str,
    repo: &str,
    args: &serde_json::Value,
) -> Result<String, String> {
    let number = args["issue_number"]
        .as_u64()
        .ok_or("issue_number is required and must be an integer for comment action.")?;
    let comment_body = args["body"].as_str().unwrap_or("");
    if comment_body.is_empty() {
        return Err("body is required for comment action.".to_string());
    }

    let path = format!("repos/{owner}/{repo}/issues/{number}/comments");
    let body = json!({ "body": comment_body });
    request(client, headers, reqwest::Method::POST, &path, Some(body)).await
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;

    fn client_and_headers() -> (reqwest::Client, reqwest::header::HeaderMap) {
        (reqwest::Client::new(), reqwest::header::HeaderMap::new())
    }

    #[test]
    fn metadata_describes_supported_actions_and_required_repository() {
        let tool = GithubIssueTool::new();
        let parameters = tool.parameters();

        assert_eq!(tool.name(), "github_issue");
        assert_eq!(
            parameters["properties"]["action"]["enum"],
            json!(["create", "list", "view", "close", "reopen", "comment"])
        );
        assert_eq!(parameters["required"], json!(["action", "owner", "repo"]));
    }

    #[tokio::test]
    async fn execute_rejects_missing_repository_before_token_or_network() {
        let result = GithubIssueTool::new()
            .execute(
                json!({ "action": "list", "owner": "owner" }),
                &ToolContext::new("."),
            )
            .await;

        assert!(result.is_error);
        assert_eq!(result.content, "owner and repo are required.");
    }

    #[tokio::test]
    async fn execute_rejects_unknown_action_without_requesting() {
        let result = GithubIssueTool::new()
            .execute(
                json!({
                    "action": "archive",
                    "owner": "owner",
                    "repo": "repo",
                    "token": "token"
                }),
                &ToolContext::new("."),
            )
            .await;

        assert!(result.is_error);
        assert_eq!(
            result.content,
            "Unknown action: 'archive'. Valid: create, list, view, close, reopen, comment"
        );
    }

    #[tokio::test]
    async fn create_requires_a_title_before_requesting() {
        let (client, headers) = client_and_headers();
        let error = issue_create(&client, &headers, "owner", "repo", &json!({}))
            .await
            .expect_err("missing title must fail");

        assert_eq!(error, "title is required for create action.");
    }

    #[tokio::test]
    async fn numbered_actions_validate_the_issue_number() {
        let (client, headers) = client_and_headers();
        let args = json!({});

        assert_eq!(
            issue_view(&client, &headers, "owner", "repo", &args)
                .await
                .expect_err("missing number must fail"),
            "issue_number is required and must be an integer for view action."
        );
        assert_eq!(
            issue_set_state(&client, &headers, "owner", "repo", &args, "closed")
                .await
                .expect_err("missing number must fail"),
            "issue_number is required and must be an integer for closed action."
        );
        assert_eq!(
            issue_comment(&client, &headers, "owner", "repo", &args)
                .await
                .expect_err("missing number must fail"),
            "issue_number is required and must be an integer for comment action."
        );
    }

    #[tokio::test]
    async fn comment_requires_a_nonempty_body_before_requesting() {
        let (client, headers) = client_and_headers();
        let error = issue_comment(
            &client,
            &headers,
            "owner",
            "repo",
            &json!({ "issue_number": 42, "body": "" }),
        )
        .await
        .expect_err("empty comment must fail");

        assert_eq!(error, "body is required for comment action.");
    }

    struct ApiBaseGuard {
        prev: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl ApiBaseGuard {
        fn set(base: &str) -> Self {
            let lock = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("WHYCODES_GITHUB_API_BASE");
            unsafe { std::env::set_var("WHYCODES_GITHUB_API_BASE", base) };
            Self { prev, _lock: lock }
        }
    }

    impl Drop for ApiBaseGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var("WHYCODES_GITHUB_API_BASE", v),
                    None => std::env::remove_var("WHYCODES_GITHUB_API_BASE"),
                }
            }
        }
    }

    fn spawn_json_server(status: &str, body: &str, n: usize) -> std::net::SocketAddr {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let payload = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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

    #[tokio::test]
    async fn execute_create_list_view_close_reopen_comment_on_loopback() {
        let addr = spawn_json_server("200 OK", r#"{"number":1}"#, 8);
        let base = format!("http://{addr}");
        let tool = GithubIssueTool;
        assert!(!tool.description().is_empty());
        let ctx = ToolContext::new(".");
        let _g = ApiBaseGuard::set(&base);
        let create = tool
            .execute(
                json!({
                    "action": "create",
                    "owner": "o",
                    "repo": "r",
                    "token": "t",
                    "title": "Bug",
                    "body": "details",
                    "labels": ["bug"]
                }),
                &ctx,
            )
            .await;
        assert!(!create.is_error, "{}", create.content);

        let list = tool
            .execute(
                json!({
                    "action": "list",
                    "owner": "o",
                    "repo": "r",
                    "token": "t",
                    "state": "all",
                    "per_page": 200
                }),
                &ctx,
            )
            .await;
        assert!(!list.is_error, "{}", list.content);

        let view = tool
            .execute(
                json!({
                    "action": "view",
                    "owner": "o",
                    "repo": "r",
                    "token": "t",
                    "issue_number": 1
                }),
                &ctx,
            )
            .await;
        assert!(!view.is_error, "{}", view.content);

        let close = tool
            .execute(
                json!({
                    "action": "close",
                    "owner": "o",
                    "repo": "r",
                    "token": "t",
                    "issue_number": 1
                }),
                &ctx,
            )
            .await;
        assert!(!close.is_error, "{}", close.content);

        let reopen = tool
            .execute(
                json!({
                    "action": "reopen",
                    "owner": "o",
                    "repo": "r",
                    "token": "t",
                    "issue_number": 1
                }),
                &ctx,
            )
            .await;
        assert!(!reopen.is_error, "{}", reopen.content);

        let comment = tool
            .execute(
                json!({
                    "action": "comment",
                    "owner": "o",
                    "repo": "r",
                    "token": "t",
                    "issue_number": 1,
                    "body": "ship it"
                }),
                &ctx,
            )
            .await;
        assert!(!comment.is_error, "{}", comment.content);
    }

    #[tokio::test]
    async fn execute_missing_token_and_api_error() {
        {
            let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("GITHUB_TOKEN");
            unsafe { std::env::remove_var("GITHUB_TOKEN") };
            let missing = GithubIssueTool::new()
                .execute(
                    json!({"action": "list", "owner": "o", "repo": "r"}),
                    &ToolContext::new("."),
                )
                .await;
            assert!(missing.is_error, "{}", missing.content);
            assert!(missing.content.contains("token"), "{}", missing.content);
            unsafe {
                match prev {
                    Some(v) => std::env::set_var("GITHUB_TOKEN", v),
                    None => std::env::remove_var("GITHUB_TOKEN"),
                }
            }
        }

        let addr = spawn_json_server("404 Not Found", "missing", 1);
        let base = format!("http://{addr}");
        let _g2 = ApiBaseGuard::set(&base);
        let err = GithubIssueTool::new()
            .execute(
                json!({
                    "action": "list",
                    "owner": "o",
                    "repo": "r",
                    "token": "t"
                }),
                &ToolContext::new("."),
            )
            .await;
        assert!(err.is_error, "{}", err.content);
        assert!(err.content.contains("GitHub API error"), "{}", err.content);

        drop(_g2);

        let mut ctx = ToolContext::new(".");
        ctx.network = whycodes_core::NetworkPolicy {
            allowlist: vec!["example.com".into()],
            denylist: vec![],
        };
        let blocked = GithubIssueTool::new()
            .execute(
                json!({"action": "list", "owner": "o", "repo": "r", "token": "t"}),
                &ctx,
            )
            .await;
        assert!(blocked.is_error, "{}", blocked.content);

        let bad_token = GithubIssueTool::new()
            .execute(
                json!({
                    "action": "list",
                    "owner": "o",
                    "repo": "r",
                    "token": "bad\ntoken"
                }),
                &ToolContext::new("."),
            )
            .await;
        assert!(bad_token.is_error, "{}", bad_token.content);
        assert!(
            bad_token.content.contains("headers"),
            "{}",
            bad_token.content
        );
    }
}
