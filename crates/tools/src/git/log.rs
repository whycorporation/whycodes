use serde_json::json;
use std::process::Command;

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

pub struct GitLogTool;

impl Default for GitLogTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GitLogTool {
    pub fn new() -> Self {
        Self
    }
}
impl Tool for GitLogTool {
    fn name(&self) -> &str {
        "git_log"
    }

    fn description(&self) -> &str {
        "Show commit logs. Runs 'git log --oneline' in the working directory."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "count": {
                    "type": "integer",
                    "description": "Number of commits to show (default: 10)"
                },
                "author": {
                    "type": "string",
                    "description": "Filter by author name or email"
                },
                "since": {
                    "type": "string",
                    "description": "Show commits more recent than a date (e.g. '2024-01-01', '1 week ago')"
                },
                "path": {
                    "type": "string",
                    "description": "Limit to commits touching a specific file or directory"
                }
            },
            "required": []
        })
    }

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            let working_dir = ctx.working_dir.clone();
            crate::blocking::tool(move || {
                let count = args["count"].as_u64().unwrap_or(10);
                let author = args["author"].as_str();
                let since = args["since"].as_str();
                let path_filter = args["path"].as_str();

                let mut cmd = Command::new("git");
                cmd.arg("log")
                    .arg("--oneline")
                    .arg("-n")
                    .arg(count.to_string());

                if let Some(a) = author {
                    cmd.arg("--author").arg(a);
                }

                if let Some(s) = since {
                    cmd.arg("--since").arg(s);
                }

                if let Some(path) = path_filter {
                    cmd.arg("--").arg(path);
                }

                cmd.current_dir(&working_dir);

                let output = match cmd.output() {
                    Ok(o) => o,
                    Err(e) => {
                        return ToolResult {
                            tool_call_id: String::new(),
                            content: format!("Failed to run git log: {}", e),
                            is_error: true,
                        };
                    }
                };

                if !output.status.success() {
                    return ToolResult {
                        tool_call_id: String::new(),
                        content: String::from_utf8_lossy(&output.stderr).to_string(),
                        is_error: true,
                    };
                }

                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let content = if stdout.is_empty() {
                    "No commits found.".to_string()
                } else {
                    stdout
                };

                ToolResult {
                    tool_call_id: String::new(),
                    content,
                    is_error: false,
                }
            })
            .await
        })
    }
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use crate::tool::ToolContext;
    use serde_json::json;
    use std::process::Command;

    #[test]
    fn log_module_loads() {
        assert!(!module_path!().is_empty());
    }

    #[tokio::test]
    async fn log_filters_and_empty() {
        let t = GitLogTool;
        assert_eq!(t.name(), "git_log");
        assert!(!t.description().is_empty());
        let _ = t.parameters();
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
        let err = t
            .execute(
                json!({"count": 1, "author": "x", "since": "2020-01-01", "path": "a.txt"}),
                &ctx,
            )
            .await;
        assert!(err.is_error, "{}", err.content);
    }

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(
            Command::new("git")
                .args(["init"])
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success()
        );
        let _ = Command::new("git")
            .args(["config", "user.email", "test@whycodes.local"])
            .current_dir(dir.path())
            .status();
        let _ = Command::new("git")
            .args(["config", "user.name", "whycodes-test"])
            .current_dir(dir.path())
            .status();
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        assert!(
            Command::new("git")
                .args(["add", "."])
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["commit", "-m", "init"])
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success()
        );
        dir
    }

    #[tokio::test]
    async fn log_no_commits_for_unmatched_author() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = init_repo();
        let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
        let out = GitLogTool::new()
            .execute(json!({"author": "nobody-xyz-unmatched"}), &ctx)
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("No commits found"), "{}", out.content);
    }

    #[tokio::test]
    async fn log_fails_when_git_missing_from_path() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("PATH");
        unsafe { std::env::set_var("PATH", "/nonexistent-whycodes-path") };
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
        let out = GitLogTool::new().execute(json!({}), &ctx).await;
        unsafe {
            match prev {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
        }
        assert!(out.is_error, "{}", out.content);
        assert!(
            out.content.contains("Failed to run git log"),
            "{}",
            out.content
        );
    }
}
