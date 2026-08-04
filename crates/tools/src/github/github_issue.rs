use async_trait::async_trait;
use serde_json::json;

use super::github_api::{api_url, github_headers, resolve_token};
use crate::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

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

#[async_trait]
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

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
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
