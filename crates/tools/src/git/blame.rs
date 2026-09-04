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

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
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
        })
    }
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use crate::tool::ToolContext;
    use serde_json::json;

    #[test]
    fn blame_module_loads() {
        assert!(!module_path!().is_empty());
    }

    #[tokio::test]
    async fn blame_line_start_only_and_missing_file() {
        let t = GitBlameTool;
        assert_eq!(t.name(), "git_blame");
        assert!(!t.description().is_empty());
        let _ = t.parameters();
        let missing = t.execute(json!({}), &ToolContext::new(".")).await;
        assert!(missing.is_error, "{}", missing.content);
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
        let err = t
            .execute(json!({"file": "a.txt", "line_start": 1}), &ctx)
            .await;
        assert!(err.is_error, "{}", err.content);
    }

    #[tokio::test]
    async fn blame_fails_when_git_missing_from_path() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("PATH");
        unsafe { std::env::set_var("PATH", "/nonexistent-whycodes-path") };
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
        let out = GitBlameTool::new()
            .execute(json!({"file": "a.txt"}), &ctx)
            .await;
        unsafe {
            match prev {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
        }
        assert!(out.is_error, "{}", out.content);
        assert!(
            out.content.contains("Failed to run git blame"),
            "{}",
            out.content
        );
    }
}
