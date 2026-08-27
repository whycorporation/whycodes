use async_trait::async_trait;
use serde_json::json;
use std::process::Command;

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

pub struct GitStatusTool;

impl Default for GitStatusTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GitStatusTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GitStatusTool {
    fn name(&self) -> &str {
        "git_status"
    }

    fn description(&self) -> &str {
        "Show the working tree status. Runs 'git status --short' in the working directory."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Limit status to a specific path"
                }
            },
            "required": []
        })
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let working_dir = ctx.working_dir.clone();
        crate::blocking::tool(move || {
            let path_filter = args["path"].as_str();

            let mut cmd = Command::new("git");
            cmd.arg("status").arg("--short");

            if let Some(path) = path_filter {
                cmd.arg("--").arg(path);
            }

            cmd.current_dir(&working_dir);

            let output = match cmd.output() {
                Ok(o) => o,
                Err(e) => {
                    return ToolResult {
                        tool_call_id: String::new(),
                        content: format!("Failed to run git status: {}", e),
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
                "Working tree clean. No changes staged or unstaged.".to_string()
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
    }
}
