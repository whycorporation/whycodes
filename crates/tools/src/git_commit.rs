use async_trait::async_trait;
use serde_json::json;
use std::process::Command;

use super::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

pub struct GitCommitTool;

impl GitCommitTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GitCommitTool {
    fn name(&self) -> &str {
        "git_commit"
    }

    fn description(&self) -> &str {
        "Stage and commit changes to git. If files are provided, runs 'git add <files>' then 'git commit -m <message>'. If no files, runs 'git commit -a -m <message>'. Optionally push with 'git push'."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "Commit message"
                },
                "files": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional list of files to stage and commit. If empty or omitted, commits all modified tracked files (git commit -a)."
                },
                "push": {
                    "type": "boolean",
                    "description": "If true, push after committing with 'git push'"
                }
            },
            "required": ["message"]
        })
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let message = match args["message"].as_str() {
            Some(m) => m,
            None => {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: "Missing required parameter: message".to_string(),
                    is_error: true,
                };
            }
        };

        let files: Vec<String> = args["files"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let push = args["push"].as_bool().unwrap_or(false);

        if !files.is_empty() {
            // Stage specific files then commit
            let mut add_cmd = Command::new("git");
            add_cmd.arg("add");
            for f in &files {
                add_cmd.arg(f);
            }
            add_cmd.current_dir(&ctx.working_dir);

            let add_output = match add_cmd.output() {
                Ok(o) => o,
                Err(e) => {
                    return ToolResult {
                        tool_call_id: String::new(),
                        content: format!("Failed to run git add: {}", e),
                        is_error: true,
                    };
                }
            };

            if !add_output.status.success() {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: String::from_utf8_lossy(&add_output.stderr).to_string(),
                    is_error: true,
                };
            }

            let mut commit_cmd = Command::new("git");
            commit_cmd.arg("commit").arg("-m").arg(message);
            commit_cmd.current_dir(&ctx.working_dir);

            let commit_output = match commit_cmd.output() {
                Ok(o) => o,
                Err(e) => {
                    return ToolResult {
                        tool_call_id: String::new(),
                        content: format!("Failed to run git commit: {}", e),
                        is_error: true,
                    };
                }
            };

            if !commit_output.status.success() {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: String::from_utf8_lossy(&commit_output.stderr).to_string(),
                    is_error: true,
                };
            }

            let result = String::from_utf8_lossy(&commit_output.stdout).to_string();
            let result = if result.is_empty() {
                "Commit succeeded (nothing to commit, possibly already committed).".to_string()
            } else {
                result
            };

            if push {
                let push_result = git_push(ctx);
                return ToolResult {
                    tool_call_id: String::new(),
                    content: format!("{}\n{}", result, push_result),
                    is_error: false,
                };
            }

            return ToolResult {
                tool_call_id: String::new(),
                content: result,
                is_error: false,
            };
        }

        // No files specified: commit all modified tracked files
        let mut cmd = Command::new("git");
        cmd.arg("commit").arg("-a").arg("-m").arg(message);
        cmd.current_dir(&ctx.working_dir);

        let output = match cmd.output() {
            Ok(o) => o,
            Err(e) => {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Failed to run git commit: {}", e),
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

        let result = String::from_utf8_lossy(&output.stdout).to_string();
        let result = if result.is_empty() {
            "Commit succeeded (nothing to commit, working tree clean).".to_string()
        } else {
            result
        };

        if push {
            let push_result = git_push(ctx);
            return ToolResult {
                tool_call_id: String::new(),
                content: format!("{}\n{}", result, push_result),
                is_error: false,
            };
        }

        ToolResult {
            tool_call_id: String::new(),
            content: result,
            is_error: false,
        }
    }
}

fn git_push(ctx: &ToolContext) -> String {
    let mut cmd = Command::new("git");
    cmd.arg("push");
    cmd.current_dir(&ctx.working_dir);

    match cmd.output() {
        Ok(o) => {
            if o.status.success() {
                let stdout = String::from_utf8_lossy(&o.stdout).to_string();
                if stdout.is_empty() {
                    "Push succeeded.".to_string()
                } else {
                    stdout
                }
            } else {
                format!("Push failed: {}", String::from_utf8_lossy(&o.stderr))
            }
        }
        Err(e) => format!("Failed to run git push: {}", e),
    }
}
