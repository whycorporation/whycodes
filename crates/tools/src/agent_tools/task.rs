use serde_json::json;

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

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
                    "description": "general (default), explore, scout, or a name from the system prompt's `# Custom agents` list"
                },
                "max_turns": {
                    "type": "integer",
                    "description": "Maximum conversation turns for the subagent (default: 15)"
                },
                "output_schema": {
                    "type": "object",
                    "description": "JSON Schema (object) for a typed result. The subagent ends with a matching ```json block (plus a required `what_i_did_not_check` string array); it is validated, retried once if invalid, returned after the summary and readable as `read agent://<id>/<json-pointer>`. Overrides a custom agent's `output`."
                }
            },
            "required": ["goal"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        _ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
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
        })
    }
}

#[cfg(test)]
#[path = "task_tests.rs"]
mod tests;
