use serde_json::json;

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

pub struct PlanTool;

impl Default for PlanTool {
    fn default() -> Self {
        Self::new()
    }
}

impl PlanTool {
    pub fn new() -> Self {
        Self
    }
}
impl Tool for PlanTool {
    fn name(&self) -> &str {
        "plan"
    }

    fn description(&self) -> &str {
        "Enter or exit planning mode (read-only, no file modifications)"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["enter", "exit"],
                    "description": "Whether to enter or exit planning mode"
                }
            },
            "required": ["action"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            let action = args["action"].as_str().unwrap_or("");

            let whycodes_dir = whycodes_core::project_dir(std::path::Path::new(&ctx.working_dir));
            let plan_mode_file = whycodes_dir.join("plan_mode");

            match action {
                "enter" => {
                    // Create .whycodes directory if it doesn't exist
                    if let Err(e) = std::fs::create_dir_all(&whycodes_dir) {
                        return plan_fs_error("Error creating .whycodes directory", e);
                    }

                    match std::fs::write(&plan_mode_file, "1") {
                        Ok(_) => plan_ok(PLAN_ENTERED),
                        Err(e) => plan_fs_error("Error entering planning mode", e),
                    }
                }
                "exit" => match std::fs::remove_file(&plan_mode_file) {
                    Ok(_) => plan_ok(PLAN_EXITED),
                    Err(e) => plan_exit_remove_error(e),
                },
                _ => plan_invalid_action(action),
            }
        })
    }
}

const PLAN_ENTERED: &str = "Planning mode entered. No file modifications will be made.";
const PLAN_EXITED: &str = "Planning mode exited. File modifications are now allowed.";
const PLAN_NOT_ACTIVE: &str = "Planning mode is not currently active (no plan_mode file found).";

fn plan_ok(content: &str) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: content.to_string(),
        is_error: false,
    }
}

fn plan_fs_error(prefix: &str, err: std::io::Error) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: format!("{prefix}: {err}"),
        is_error: true,
    }
}

fn plan_exit_remove_error(err: std::io::Error) -> ToolResult {
    if err.kind() == std::io::ErrorKind::NotFound {
        plan_ok(PLAN_NOT_ACTIVE)
    } else {
        plan_fs_error("Error exiting planning mode", err)
    }
}

fn plan_invalid_action(action: &str) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: format!("Invalid action: '{action}'. Must be 'enter' or 'exit'."),
        is_error: true,
    }
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
