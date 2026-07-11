use async_trait::async_trait;
use serde_json::json;

use super::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

pub struct TaskTool;

impl Default for TaskTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &str {
        "task"
    }

    fn description(&self) -> &str {
        "Launch a subagent to handle a complex multi-step task. Use for research, exploration, or parallel units of work. Prefer explore for read-only codebase search, general for multi-step work that may edit files."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "goal": {
                    "type": "string",
                    "description": "The goal or task for the subagent to accomplish"
                },
                "context": {
                    "type": "string",
                    "description": "Additional context for the subagent (optional)"
                },
                "subagent_type": {
                    "type": "string",
                    "enum": ["general", "explore", "scout"],
                    "description": "Which subagent to spawn (default: general)"
                },
                "max_turns": {
                    "type": "integer",
                    "description": "Maximum conversation turns for the subagent (default: 15)"
                }
            },
            "required": ["goal"]
        })
    }

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        // Real execution is intercepted by Agent::run_turn (needs provider/model/api_key).
        // If this path is hit, the agent loop failed to intercept.
        let goal = args["goal"].as_str().unwrap_or("(no goal specified)");
        ToolResult {
            tool_call_id: String::new(),
            content: format!(
                "Task tool was not intercepted by the agent loop. Goal was: {}",
                goal
            ),
            is_error: true,
        }
    }
}
