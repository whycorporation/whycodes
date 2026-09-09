use serde_json::json;
use std::process::Command;

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

pub struct GitCommitTool;

impl Default for GitCommitTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GitCommitTool {
    pub fn new() -> Self {
        Self
    }
}
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

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            let message = match args["message"].as_str() {
                Some(m) => m.to_string(),
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
            let working_dir = ctx.working_dir.clone();
            crate::blocking::tool(move || Self::run(working_dir, message, files, push)).await
        })
    }
}

impl GitCommitTool {
    fn run(working_dir: String, message: String, files: Vec<String>, push: bool) -> ToolResult {
        if !files.is_empty() {
            let mut add_cmd = Command::new("git");
            add_cmd.arg("add");
            for f in &files {
                add_cmd.arg(f);
            }
            add_cmd.current_dir(&working_dir);

            if let Err(e) = git_output(add_cmd.output(), "Failed to run git add") {
                return e;
            }

            let mut commit_cmd = Command::new("git");
            commit_cmd.arg("commit").arg("-m").arg(&message);
            commit_cmd.current_dir(&working_dir);

            return commit_from_output(
                take_git_output(git_output(commit_cmd.output(), "Failed to run git commit")),
                "Commit succeeded (nothing to commit, possibly already committed).",
                push,
                &working_dir,
            );
        }

        let mut cmd = Command::new("git");
        cmd.arg("commit").arg("-a").arg("-m").arg(&message);
        cmd.current_dir(&working_dir);

        commit_from_output(
            take_git_output(git_output(cmd.output(), "Failed to run git commit")),
            "Commit succeeded (nothing to commit, working tree clean).",
            push,
            &working_dir,
        )
    }
}

fn take_git_output(
    result: Result<std::process::Output, ToolResult>,
) -> Result<std::process::Output, ToolResult> {
    match result {
        Ok(o) => Ok(o),
        Err(e) => Err(git_output_failed(e)),
    }
}

fn commit_from_output(
    result: Result<std::process::Output, ToolResult>,
    empty: &str,
    push: bool,
    working_dir: &str,
) -> ToolResult {
    match result {
        Ok(output) => {
            let result = commit_stdout(&output.stdout, empty);
            if push {
                let push_result = git_push(working_dir);
                ToolResult {
                    tool_call_id: String::new(),
                    content: format!("{}\n{}", result, push_result),
                    is_error: false,
                }
            } else {
                ToolResult {
                    tool_call_id: String::new(),
                    content: result,
                    is_error: false,
                }
            }
        }
        Err(e) => e,
    }
}

fn git_output_failed(e: ToolResult) -> ToolResult {
    e
}

fn git_output(
    result: std::io::Result<std::process::Output>,
    prefix: &str,
) -> Result<std::process::Output, ToolResult> {
    match result {
        Ok(o) if o.status.success() => Ok(o),
        Ok(o) => Err(git_stderr_error(&o.stderr)),
        Err(e) => Err(git_cmd_error(prefix, &e.to_string())),
    }
}

fn git_cmd_error(prefix: &str, e: &str) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: format!("{prefix}: {e}"),
        is_error: true,
    }
}

fn git_stderr_error(stderr: &[u8]) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: String::from_utf8_lossy(stderr).to_string(),
        is_error: true,
    }
}

fn commit_stdout(stdout: &[u8], empty: &str) -> String {
    let result = String::from_utf8_lossy(stdout).to_string();
    if result.is_empty() {
        empty.to_string()
    } else {
        result
    }
}

fn git_push(working_dir: &str) -> String {
    let mut cmd = Command::new("git");
    cmd.arg("push");
    cmd.current_dir(working_dir);

    match cmd.output() {
        Ok(o) => {
            if o.status.success() {
                commit_stdout(&o.stdout, "Push succeeded.")
            } else {
                format!("Push failed: {}", String::from_utf8_lossy(&o.stderr))
            }
        }
        Err(e) => format!("Failed to run git push: {}", e),
    }
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
#[path = "commit_tests.rs"]
mod tests;
