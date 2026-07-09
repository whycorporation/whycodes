use async_trait::async_trait;
use serde_json::json;
use std::process::Command;

use super::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

pub struct GitDiffTool;

impl GitDiffTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
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

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
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

        cmd.current_dir(&ctx.working_dir);

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
    }
}
