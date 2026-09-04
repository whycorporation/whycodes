use serde_json::json;
use std::process::Command;

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

pub struct GitDiffTool;

impl Default for GitDiffTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GitDiffTool {
    pub fn new() -> Self {
        Self
    }
}
impl Tool for GitDiffTool {
    fn name(&self) -> &str {
        "git_diff"
    }

    fn description(&self) -> &str {
        "Show changes between commits, commit and working tree, etc. Runs 'git diff' in the working directory."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "staged": {
                    "type": "boolean",
                    "description": "Show staged changes (--staged / --cached)"
                },
                "path": {
                    "type": "string",
                    "description": "Limit diff to a specific file or directory"
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
                let staged = args["staged"].as_bool().unwrap_or(false);
                let path_filter = args["path"].as_str();

                let mut cmd = Command::new("git");
                cmd.arg("diff");

                if staged {
                    cmd.arg("--staged");
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
                            content: format!("Failed to run git diff: {}", e),
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
                    "No changes (working tree clean).".to_string()
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
    fn diff_module_loads() {
        assert!(!module_path!().is_empty());
    }

    #[tokio::test]
    async fn diff_staged_and_path() {
        let t = GitDiffTool;
        assert_eq!(t.name(), "git_diff");
        assert!(!t.description().is_empty());
        let _ = t.parameters();
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
        let err = t
            .execute(json!({"staged": true, "path": "a.txt"}), &ctx)
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
    async fn diff_clean_tree_and_path_filter() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = init_repo();
        let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
        let clean = GitDiffTool::new().execute(json!({}), &ctx).await;
        assert!(!clean.is_error, "{}", clean.content);
        assert!(
            clean.content.contains("No changes") || clean.content.is_empty(),
            "{}",
            clean.content
        );
        std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        let filtered = GitDiffTool::new()
            .execute(json!({"path": "a.txt", "staged": false}), &ctx)
            .await;
        assert!(!filtered.is_error, "{}", filtered.content);
    }

    #[tokio::test]
    async fn diff_fails_when_git_missing_from_path() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("PATH");
        unsafe { std::env::set_var("PATH", "/nonexistent-whycodes-path") };
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
        let out = GitDiffTool::new().execute(json!({}), &ctx).await;
        unsafe {
            match prev {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
        }
        assert!(out.is_error, "{}", out.content);
        assert!(
            out.content.contains("Failed to run git diff"),
            "{}",
            out.content
        );
    }
}
