use async_trait::async_trait;
use serde_json::json;

use super::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

pub struct TaskTool;

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
        "Delegate a task to a subagent"
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
                }
            },
            "required": ["goal"]
        })
    }

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let goal = args["goal"].as_str().unwrap_or("(no goal specified)");
        let context = args.get("context").and_then(|v| v.as_str());

        let mut response = format!(
            "Task delegation not yet wired. Would delegate:\n  Goal: {}\n",
            goal
        );
        if let Some(ctx) = context {
            response.push_str(&format!("  Context: {}\n", ctx));
        }
        response.push_str("  Status: placeholder - real subagent spawning will be connected in a future update.");

        ToolResult {
            tool_call_id: String::new(),
            content: response,
            is_error: false,
        }
    }
}
