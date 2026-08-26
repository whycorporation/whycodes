use async_trait::async_trait;
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

#[async_trait]
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

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let action = args["action"].as_str().unwrap_or("");

        let whycodes_dir = whycodes_core::project_dir(std::path::Path::new(&ctx.working_dir));
        let plan_mode_file = whycodes_dir.join("plan_mode");

        match action {
            "enter" => {
                // Create .whycodes directory if it doesn't exist
                if let Err(e) = std::fs::create_dir_all(&whycodes_dir) {
                    return ToolResult {
                        tool_call_id: String::new(),
                        content: format!("Error creating .whycodes directory: {}", e),
                        is_error: true,
                    };
                }

                match std::fs::write(&plan_mode_file, "1") {
                    Ok(_) => ToolResult {
                        tool_call_id: String::new(),
                        content: "Planning mode entered. No file modifications will be made."
                            .to_string(),
                        is_error: false,
                    },
                    Err(e) => ToolResult {
                        tool_call_id: String::new(),
                        content: format!("Error entering planning mode: {}", e),
                        is_error: true,
                    },
                }
            }
            "exit" => match std::fs::remove_file(&plan_mode_file) {
                Ok(_) => ToolResult {
                    tool_call_id: String::new(),
                    content: "Planning mode exited. File modifications are now allowed."
                        .to_string(),
                    is_error: false,
                },
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        ToolResult {
                            tool_call_id: String::new(),
                            content:
                                "Planning mode is not currently active (no plan_mode file found)."
                                    .to_string(),
                            is_error: false,
                        }
                    } else {
                        ToolResult {
                            tool_call_id: String::new(),
                            content: format!("Error exiting planning mode: {}", e),
                            is_error: true,
                        }
                    }
                }
            },
            _ => ToolResult {
                tool_call_id: String::new(),
                content: format!("Invalid action: '{}'. Must be 'enter' or 'exit'.", action),
                is_error: true,
            },
        }
    }
}
