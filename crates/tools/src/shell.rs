use async_trait::async_trait;
use serde_json::json;
use std::process::Command;

use super::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

pub struct ShellTool;

impl ShellTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute a shell command in the terminal and return its output."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (default: 120)"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let command_str = args["command"].as_str().unwrap_or("");
        let timeout_secs = args["timeout"].as_u64().unwrap_or(120) as u64;

        let result = tokio::task::spawn_blocking({
            let command_str = command_str.to_string();
            let working_dir = ctx.working_dir.clone();
            move || {
                let output = Command::new("bash")
                    .arg("-c")
                    .arg(&command_str)
                    .current_dir(&working_dir)
                    .output();

                match output {
                    Ok(out) => {
                        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                        let mut result = String::new();

                        if !stdout.is_empty() {
                            result.push_str(&stdout);
                        }
                        if !stderr.is_empty() {
                            if !result.is_empty() {
                                result.push('\n');
                            }
                            result.push_str("[stderr]\n");
                            result.push_str(&stderr);
                        }

                        if result.is_empty() {
                            result = format!(
                                "Command executed successfully (exit code: {})",
                                out.status.code().unwrap_or(0)
                            );
                        }

                        (result, out.status.success())
                    }
                    Err(e) => (format!("Error executing command: {}", e), false),
                }
            }
        });

        match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), result).await {
            Ok(Ok((content, success))) => ToolResult {
                tool_call_id: String::new(),
                content,
                is_error: !success,
            },
            Ok(Err(e)) => ToolResult {
                tool_call_id: String::new(),
                content: format!("Task join error: {}", e),
                is_error: true,
            },
            Err(_) => ToolResult {
                tool_call_id: String::new(),
                content: format!(
                    "Command timed out after {} seconds",
                    timeout_secs
                ),
                is_error: true,
            },
        }
    }
}
