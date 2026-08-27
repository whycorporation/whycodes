use async_trait::async_trait;
use serde_json::json;
use std::process::Command;

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

pub struct GitBlameTool;

impl Default for GitBlameTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GitBlameTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GitBlameTool {
    fn name(&self) -> &str {
        "git_blame"
    }

    fn description(&self) -> &str {
        "Show what revision and author last modified each line of a file. Runs 'git blame' in the working directory."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file": {
                    "type": "string",
                    "description": "Path to the file to blame (required)"
                },
                "revision": {
                    "type": "string",
                    "description": "Revision to blame against (default: HEAD)"
                },
                "line_start": {
                    "type": "integer",
                    "description": "Start line number (1-based) to limit blame range"
                },
                "line_end": {
                    "type": "integer",
                    "description": "End line number (1-based) to limit blame range"
                }
            },
            "required": ["file"]
        })
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let file = match args["file"].as_str() {
            Some(f) => f.to_string(),
            None => {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: "Missing required parameter: 'file'".to_string(),
                    is_error: true,
                };
            }
        };

        let working_dir = ctx.working_dir.clone();
        crate::blocking::tool(move || {
            let mut cmd = Command::new("git");
            cmd.arg("blame");

            if let Some(rev) = args["revision"].as_str() {
                cmd.arg(rev);
            }

            let line_start = args["line_start"].as_u64();
            let line_end = args["line_end"].as_u64();

            if let (Some(start), Some(end)) = (line_start, line_end) {
                cmd.arg("-L").arg(format!("{},{}", start, end));
            } else if let Some(start) = line_start {
                cmd.arg("-L").arg(format!("{},", start));
            }

            cmd.arg("--").arg(&file);
            cmd.current_dir(&working_dir);

            let output = match cmd.output() {
                Ok(o) => o,
                Err(e) => {
                    return ToolResult {
                        tool_call_id: String::new(),
                        content: format!("Failed to run git blame: {}", e),
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
                "No blame information available.".to_string()
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
