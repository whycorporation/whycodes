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
mod tests {
    use super::*;
    use crate::tool::ToolContext;

    #[test]
    fn task_module_loads() {
        assert!(!module_path!().is_empty());
    }

    #[tokio::test]
    async fn execute_reports_missing_intercept() {
        let t = TaskTool;
        assert_eq!(t.name(), "task");
        assert!(!t.description().is_empty());
        assert_eq!(t.parameters()["required"][0], "goal");
        let r = t
            .execute(
                serde_json::json!({"goal": "explore"}),
                &ToolContext::new("."),
            )
            .await;
        assert!(r.is_error);
        assert!(r.content.contains("not intercepted"), "{}", r.content);
        assert!(r.content.contains("explore"), "{}", r.content);
        let missing = t
            .execute(serde_json::json!({}), &ToolContext::new("."))
            .await;
        assert!(
            missing.content.contains("no goal specified"),
            "{}",
            missing.content
        );
    }
}
