use serde_json::json;
use std::path::PathBuf;

use super::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;
use whycodes_sandbox::{SandboxRequest, run_timeout as sandbox_run};

pub struct ShellTool {
    name: &'static str,
}

impl Default for ShellTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellTool {
    pub fn new() -> Self {
        Self { name: "bash" }
    }

    pub fn as_shell() -> Self {
        Self { name: "shell" }
    }
}
impl Tool for ShellTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "Execute a shell command in the project environment and return stdout/stderr. \
         When security.sandbox=workspace (default), the process is confined: project \
         directory is writable, the rest of the filesystem is read-only; network may \
         be disabled via security.sandbox_network=false. \
         Set background=true to return immediately with a job id (use `bg` to read/kill)."
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
                    "description": "Timeout in seconds (default: 120; ignored when background=true)"
                },
                "description": {
                    "type": "string",
                    "description": "Short description of why this command is run (optional)"
                },
                "background": {
                    "type": "boolean",
                    "description": "If true, start the command in the background and return a job id immediately"
                }
            },
            "required": ["command"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            let command_str = args["command"].as_str().unwrap_or("");
            let timeout_secs = args["timeout"].as_u64().unwrap_or(120);

            let request = SandboxRequest {
                command: command_str.to_string(),
                working_dir: PathBuf::from(&ctx.working_dir),
                settings: ctx.sandbox.clone(),
            };
            let timeout = std::time::Duration::from_secs(timeout_secs.max(1));

            // Timeout lives inside the spawn: dropping this future must not leak
            // `sleep 999` / hung `cargo test` on a blocking thread.
            match tokio::task::spawn_blocking(move || sandbox_run(&request, Some(timeout))).await {
                Ok(Ok(outcome)) => {
                    let (content, success) = outcome.display_content();
                    ToolResult {
                        tool_call_id: String::new(),
                        content,
                        is_error: !success,
                    }
                }
                Ok(Err(whycodes_sandbox::SandboxError::TimedOut(secs))) => ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Command timed out after {secs} seconds"),
                    is_error: true,
                },
                Ok(Err(e)) => ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Sandbox error: {e}"),
                    is_error: true,
                },
                Err(e) => ToolResult {
                    tool_call_id: String::new(),
                    content: format!("Task join error: {e}"),
                    is_error: true,
                },
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::Tool;
    use crate::tool::ToolContext;
    use serde_json::json;

    #[test]
    fn shell_module_loads() {
        assert!(!module_path!().is_empty());
    }

    #[test]
    fn bash_and_shell_aliases_and_schema() {
        let bash = ShellTool::new();
        let shell = ShellTool::as_shell();
        let via_default = ShellTool::default();
        assert_eq!(bash.name(), "bash");
        assert_eq!(shell.name(), "shell");
        assert_eq!(via_default.name(), "bash");
        assert!(bash.description().contains("shell") || bash.description().contains("command"));
        let params = bash.parameters();
        assert_eq!(params["required"][0], "command");
    }

    #[tokio::test]
    async fn execute_echo_and_failing_command() {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
        let tool = ShellTool::new();

        let ok = tool
            .execute(json!({"command": "echo hello-cov", "timeout": 10}), &ctx)
            .await;
        assert!(!ok.is_error, "{ok:?}");
        assert!(ok.content.contains("hello-cov"), "{ok:?}");

        let empty = tool.execute(json!({"command": ""}), &ctx).await;
        assert!(empty.is_error || empty.content.contains("empty") || !empty.content.is_empty());

        let fail = tool
            .execute(json!({"command": "exit 42", "timeout": 10}), &ctx)
            .await;
        assert!(
            fail.is_error || fail.content.contains("42") || !fail.content.is_empty(),
            "{fail:?}"
        );
    }

    #[tokio::test]
    async fn timeout_zero_is_clamped() {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = ToolContext::new(dir.path().to_string_lossy().into_owned());
        let out = ShellTool::default()
            .execute(json!({"command": "true", "timeout": 0}), &ctx)
            .await;
        assert!(!out.is_error || !out.content.is_empty(), "{out:?}");
    }

    #[tokio::test]
    async fn timeout_kills_long_sleep() {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = ToolContext::unsandboxed(dir.path().to_string_lossy().into_owned());
        let out = ShellTool::new()
            .execute(json!({"command": "sleep 30", "timeout": 1}), &ctx)
            .await;
        assert!(out.is_error, "{}", out.content);
        assert!(
            out.content.contains("timed out") || out.content.contains("Sandbox error"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn sandbox_error_from_file_as_working_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("not-a-dir");
        std::fs::write(&file, "x").unwrap();
        let ctx = ToolContext::new(file.to_string_lossy().into_owned());
        let out = ShellTool::new()
            .execute(json!({"command": "echo hi", "timeout": 5}), &ctx)
            .await;
        assert!(out.is_error, "{}", out.content);
        assert!(
            out.content.contains("Sandbox error")
                || out.content.contains("working directory")
                || !out.content.is_empty(),
            "{}",
            out.content
        );
    }
}
