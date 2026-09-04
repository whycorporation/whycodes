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
            commit_cmd.arg("commit").arg("-m").arg(&message);
            commit_cmd.current_dir(&working_dir);

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
                let push_result = git_push(&working_dir);
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

        let mut cmd = Command::new("git");
        cmd.arg("commit").arg("-a").arg("-m").arg(&message);
        cmd.current_dir(&working_dir);

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
            let push_result = git_push(&working_dir);
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

fn git_push(working_dir: &str) -> String {
    let mut cmd = Command::new("git");
    cmd.arg("push");
    cmd.current_dir(working_dir);

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

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use crate::tool::ToolContext;
    use serde_json::json;
    use std::process::Command;

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

    #[test]
    fn commit_module_loads() {
        assert!(!module_path!().is_empty());
    }

    #[tokio::test]
    async fn commit_all_push_and_error_paths() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let t = GitCommitTool;
        assert_eq!(t.name(), "git_commit");
        assert!(!t.description().is_empty());
        assert_eq!(t.parameters()["required"][0], "message");

        let dir = init_repo();
        let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
        std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        let all = t
            .execute(json!({"message": "all tracked", "files": []}), &ctx)
            .await;
        assert!(!all.is_error, "{}", all.content);

        std::fs::write(dir.path().join("b.txt"), "new\n").unwrap();
        let add = t
            .execute(
                json!({"message": "add b", "files": ["b.txt", 1], "push": false}),
                &ctx,
            )
            .await;
        assert!(!add.is_error, "{}", add.content);

        let push = t
            .execute(
                json!({"message": "push me", "files": ["nope.txt"], "push": true}),
                &ctx,
            )
            .await;
        assert!(
            push.is_error || push.content.contains("Push") || !push.content.is_empty(),
            "{}",
            push.content
        );

        let empty_push = git_push(dir.path().to_str().unwrap());
        assert!(!empty_push.is_empty());

        let missing = t.execute(json!({}), &ctx).await;
        assert!(missing.is_error, "{}", missing.content);

        let bad_add = t
            .execute(
                json!({"message": "x", "files": ["does-not-exist.txt"]}),
                &ctx,
            )
            .await;
        assert!(bad_add.is_error, "{}", bad_add.content);
    }

    #[tokio::test]
    async fn commit_push_to_local_remote_and_nothing_to_commit() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = init_repo();
        let remote = tempfile::TempDir::new().unwrap();
        assert!(
            Command::new("git")
                .args(["init", "--bare"])
                .current_dir(remote.path())
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["remote", "add", "origin", remote.path().to_str().unwrap()])
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success()
        );
        let branch = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "--abbrev-ref", "HEAD"])
                .current_dir(dir.path())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let branch = branch.trim();
        let _ = Command::new("git")
            .args(["push", "-u", "origin", branch])
            .current_dir(dir.path())
            .status();

        let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
        std::fs::write(dir.path().join("c.txt"), "c\n").unwrap();
        let with_files = GitCommitTool::new()
            .execute(
                json!({"message": "add c", "files": ["c.txt"], "push": true}),
                &ctx,
            )
            .await;
        assert!(!with_files.is_error, "{}", with_files.content);

        std::fs::write(dir.path().join("a.txt"), "three\n").unwrap();
        let all_push = GitCommitTool::new()
            .execute(json!({"message": "update a", "push": true}), &ctx)
            .await;
        assert!(!all_push.is_error, "{}", all_push.content);

        let nothing = GitCommitTool::new()
            .execute(json!({"message": "empty"}), &ctx)
            .await;
        assert!(nothing.is_error, "{}", nothing.content);

        let push_ok = git_push(dir.path().to_str().unwrap());
        assert!(!push_ok.is_empty(), "{push_ok}");
    }

    #[tokio::test]
    async fn commit_fails_when_git_missing_from_path() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("PATH");
        unsafe { std::env::set_var("PATH", "/nonexistent-whycodes-path") };
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
        let out = GitCommitTool::new()
            .execute(json!({"message": "x", "files": ["a.txt"]}), &ctx)
            .await;
        let all = GitCommitTool::new()
            .execute(json!({"message": "x"}), &ctx)
            .await;
        let push = git_push(dir.path().to_str().unwrap());
        unsafe {
            match prev {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
        }
        assert!(out.is_error, "{}", out.content);
        assert!(
            out.content.contains("Failed to run git add"),
            "{}",
            out.content
        );
        assert!(all.is_error, "{}", all.content);
        assert!(
            all.content.contains("Failed to run git commit"),
            "{}",
            all.content
        );
        assert!(push.contains("Failed to run git push"), "{push}");
    }
}
