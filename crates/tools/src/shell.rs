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
    #[test]
    fn shell_module_loads() {
        assert!(!module_path!().is_empty());
    }
}
