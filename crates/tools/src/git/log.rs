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
mod tests {
    #[test]
    fn log_module_loads() {
        assert!(!module_path!().is_empty());
    }
}
