use async_trait::async_trait;
use serde_json::{Value, json};

use super::api;
use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

pub struct GitHubPrTool;

impl Default for GitHubPrTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GitHubPrTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GitHubPrTool {
    fn name(&self) -> &str {
        "github_pr"
    }

    fn description(&self) -> &str {
        "Create, list, view, or merge GitHub Pull Requests via the REST API. \
         Requires a GitHub token (passed as parameter or set via GITHUB_TOKEN env var)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "Action to perform on pull requests",
                    "enum": ["create", "list", "view", "merge"]
                },
                "owner": {
                    "type": "string",
                    "description": "Repository owner (user or organization)"
                },
                "repo": {
                    "type": "string",
                    "description": "Repository name"
                },
                "token": {
                    "type": "string",
                    "description": "GitHub personal access token (optional; falls back to GITHUB_TOKEN env var)"
                },
                "title": {
                    "type": "string",
                    "description": "[create] PR title"
                },
                "head": {
                    "type": "string",
                    "description": "[create] Branch name containing the changes (e.g. 'feature-branch')"
                },
                "base": {
                    "type": "string",
                    "description": "[create] Target branch for the PR (e.g. 'main')"
                },
                "body": {
                    "type": "string",
                    "description": "[create] PR description/body text"
                },
                "state": {
                    "type": "string",
                    "description": "[list] Filter PRs by state",
                    "enum": ["open", "closed", "all"]
                },
                "pr_number": {
                    "type": "integer",
                    "description": "[view | merge] Pull request number"
                },
                "method": {
                    "type": "string",
                    "description": "[merge] Merge method to use",
                    "enum": ["merge", "squash", "rebase"]
                }
            },
            "required": ["action", "owner", "repo"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> ToolResult {
        let action = args["action"].as_str().unwrap_or("");
        let owner = args["owner"].as_str().unwrap_or("");
        let repo = args["repo"].as_str().unwrap_or("");

        // Validate required fields
        if owner.is_empty() || repo.is_empty() {
            return ToolResult {
                tool_call_id: String::new(),
                content: "Both 'owner' and 'repo' are required.".to_string(),
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

        // Resolve token
        let explicit_token = args["token"].as_str();
        let token = match api::resolve_token(explicit_token) {
            Some(t) => t,
            None => {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: "GitHub token is required. Provide via the 'token' parameter or set the GITHUB_TOKEN environment variable.".to_string(),
                    is_error: true,
                };
            }
        };

        let client = reqwest::Client::new();

        match action {
            "create" => Self::create_pr(&client, &token, owner, repo, &args).await,
            "list" => Self::list_prs(&client, &token, owner, repo, &args).await,
            "view" => Self::view_pr(&client, &token, owner, repo, &args).await,
            "merge" => Self::merge_pr(&client, &token, owner, repo, &args).await,
            _ => ToolResult {
                tool_call_id: String::new(),
                content: format!(
                    "Unknown action '{}'. Valid actions: create, list, view, merge.",
                    action
                ),
                is_error: true,
            },
        }
    }
}

impl GitHubPrTool {
    /// Create a new pull request.
    /// POST /repos/{owner}/{repo}/pulls
    async fn create_pr(
        client: &reqwest::Client,
        token: &str,
        owner: &str,
        repo: &str,
        args: &Value,
    ) -> ToolResult {
        let title = args["title"].as_str().unwrap_or("");
        let head = args["head"].as_str().unwrap_or("");
        let base = args["base"].as_str().unwrap_or("");
        let body = args["body"].as_str().unwrap_or("");

        if title.is_empty() || head.is_empty() || base.is_empty() {
            return ToolResult {
                tool_call_id: String::new(),
                content: "For 'create' action, 'title', 'head', and 'base' are required."
                    .to_string(),
                is_error: true,
            };
        }

        let payload = json!({
            "title": title,
            "head": head,
            "base": base,
            "body": body,
        });

        let path = format!("repos/{owner}/{repo}/pulls");
        match api::make_request(client, reqwest::Method::POST, &path, token, Some(payload)).await {
            Ok((status, text)) => {
                let is_error = !status.is_success();
                // Try to pretty-print the JSON response
                let formatted = if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    serde_json::to_string_pretty(&v).unwrap_or(text)
                } else {
                    text
                };
                ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Status: {}\n\n{}", status.as_u16(), formatted),
                    is_error,
                }
            }
            Err(e) => ToolResult {
                tool_call_id: String::new(),
                content: format!("Error creating PR: {}", e),
                is_error: true,
            },
        }
    }

    /// List pull requests for a repository.
    /// GET /repos/{owner}/{repo}/pulls?state={state}
    async fn list_prs(
        client: &reqwest::Client,
        token: &str,
        owner: &str,
        repo: &str,
        args: &Value,
    ) -> ToolResult {
        let state = args["state"].as_str().unwrap_or("open");
        let path = format!(
            "repos/{owner}/{repo}/pulls?state={state}&sort=updated&direction=desc&per_page=30"
        );

        match api::make_request(client, reqwest::Method::GET, &path, token, None).await {
            Ok((status, text)) => {
                let is_error = !status.is_success();
                let formatted = if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    serde_json::to_string_pretty(&v).unwrap_or(text)
                } else {
                    text
                };
                ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Status: {}\n\n{}", status.as_u16(), formatted),
                    is_error,
                }
            }
            Err(e) => ToolResult {
                tool_call_id: String::new(),
                content: format!("Error listing PRs: {}", e),
                is_error: true,
            },
        }
    }

    /// View a single pull request.
    /// GET /repos/{owner}/{repo}/pulls/{pull_number}
    async fn view_pr(
        client: &reqwest::Client,
        token: &str,
        owner: &str,
        repo: &str,
        args: &Value,
    ) -> ToolResult {
        let pr_number = match args["pr_number"].as_u64() {
            Some(n) => n,
            None => {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: "For 'view' action, 'pr_number' is required.".to_string(),
                    is_error: true,
                };
            }
        };

        let path = format!("repos/{owner}/{repo}/pulls/{pr_number}");
        match api::make_request(client, reqwest::Method::GET, &path, token, None).await {
            Ok((status, text)) => {
                let is_error = !status.is_success();
                let formatted = if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    serde_json::to_string_pretty(&v).unwrap_or(text)
                } else {
                    text
                };
                ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Status: {}\n\n{}", status.as_u16(), formatted),
                    is_error,
                }
            }
            Err(e) => ToolResult {
                tool_call_id: String::new(),
                content: format!("Error viewing PR: {}", e),
                is_error: true,
            },
        }
    }

    /// Merge a pull request.
    /// PUT /repos/{owner}/{repo}/pulls/{pull_number}/merge
    async fn merge_pr(
        client: &reqwest::Client,
        token: &str,
        owner: &str,
        repo: &str,
        args: &Value,
    ) -> ToolResult {
        let pr_number = match args["pr_number"].as_u64() {
            Some(n) => n,
            None => {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: "For 'merge' action, 'pr_number' is required.".to_string(),
                    is_error: true,
                };
            }
        };

        let method = args["method"].as_str().unwrap_or("merge");
        if !["merge", "squash", "rebase"].contains(&method) {
            return ToolResult {
                tool_call_id: String::new(),
                content: format!(
                    "Invalid merge method '{}'. Valid methods: merge, squash, rebase.",
                    method
                ),
                is_error: true,
            };
        }

        let payload = json!({
            "merge_method": method,
        });

        let path = format!("repos/{owner}/{repo}/pulls/{pr_number}/merge");
        match api::make_request(client, reqwest::Method::PUT, &path, token, Some(payload)).await {
            Ok((status, text)) => {
                let is_error = !status.is_success();
                let formatted = if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    serde_json::to_string_pretty(&v).unwrap_or(text)
                } else {
                    text
                };
                ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Status: {}\n\n{}", status.as_u16(), formatted),
                    is_error,
                }
            }
            Err(e) => ToolResult {
                tool_call_id: String::new(),
                content: format!("Error merging PR: {}", e),
                is_error: true,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_describes_supported_actions_and_required_repository() {
        let tool = GitHubPrTool::new();
        let parameters = tool.parameters();

        assert_eq!(tool.name(), "github_pr");
        assert_eq!(
            parameters["properties"]["action"]["enum"],
            json!(["create", "list", "view", "merge"])
        );
        assert_eq!(parameters["required"], json!(["action", "owner", "repo"]));
    }

    #[tokio::test]
    async fn execute_rejects_missing_repository_before_token_or_network() {
        let result = GitHubPrTool::new()
            .execute(
                json!({ "action": "list", "owner": "owner" }),
                &ToolContext::new("."),
            )
            .await;

        assert!(result.is_error);
        assert_eq!(result.content, "Both 'owner' and 'repo' are required.");
    }

    #[tokio::test]
    async fn execute_rejects_unknown_action_without_requesting() {
        let result = GitHubPrTool::new()
            .execute(
                json!({
                    "action": "close",
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
            "Unknown action 'close'. Valid actions: create, list, view, merge."
        );
    }

    #[tokio::test]
    async fn create_validates_required_fields_before_requesting() {
        let result = GitHubPrTool::create_pr(
            &reqwest::Client::new(),
            "token",
            "owner",
            "repo",
            &json!({ "title": "Title", "head": "feature" }),
        )
        .await;

        assert!(result.is_error);
        assert_eq!(
            result.content,
            "For 'create' action, 'title', 'head', and 'base' are required."
        );
    }

    #[tokio::test]
    async fn view_and_merge_require_a_pull_request_number() {
        let client = reqwest::Client::new();
        let args = json!({});

        let view = GitHubPrTool::view_pr(&client, "token", "owner", "repo", &args).await;
        assert!(view.is_error);
        assert_eq!(view.content, "For 'view' action, 'pr_number' is required.");

        let merge = GitHubPrTool::merge_pr(&client, "token", "owner", "repo", &args).await;
        assert!(merge.is_error);
        assert_eq!(
            merge.content,
            "For 'merge' action, 'pr_number' is required."
        );
    }

    #[tokio::test]
    async fn merge_rejects_an_invalid_method_before_requesting() {
        let result = GitHubPrTool::merge_pr(
            &reqwest::Client::new(),
            "token",
            "owner",
            "repo",
            &json!({ "pr_number": 7, "method": "fast-forward" }),
        )
        .await;

        assert!(result.is_error);
        assert_eq!(
            result.content,
            "Invalid merge method 'fast-forward'. Valid methods: merge, squash, rebase."
        );
    }
}
