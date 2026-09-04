use serde_json::json;

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

/// Delay then run a shell command in the background and/or enqueue a user prompt.
pub struct ScheduleTool;

impl Default for ScheduleTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ScheduleTool {
    pub fn new() -> Self {
        Self
    }
}
impl Tool for ScheduleTool {
    fn name(&self) -> &str {
        "schedule"
    }

    fn description(&self) -> &str {
        "After `after_secs`, either start a background shell `command` and/or queue a \
         user `goal` prompt for the next free turn. Use for deferred checks and simple automation. \
         Not a persistent daemon — process-local only."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "after_secs": {
                    "type": "integer",
                    "description": "Delay in seconds before firing (default 0, max 86400)"
                },
                "command": {
                    "type": "string",
                    "description": "Shell command to run in the background when the timer fires"
                },
                "goal": {
                    "type": "string",
                    "description": "User prompt to enqueue for the next free agent turn"
                },
                "description": {
                    "type": "string",
                    "description": "Short label for the scheduled job"
                }
            }
        })
    }

    fn execute<'a>(
        &'a self,
        _args: serde_json::Value,
        _ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            ToolResult {
                tool_call_id: String::new(),
                content: "schedule tool was not intercepted by the agent loop.".into(),
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
    fn schedule_module_loads() {
        assert!(!module_path!().is_empty());
    }

    #[tokio::test]
    async fn execute_reports_missing_intercept() {
        let t = ScheduleTool;
        assert_eq!(t.name(), "schedule");
        assert!(!t.description().is_empty());
        let _ = t.parameters();
        let r = t
            .execute(serde_json::json!({}), &ToolContext::new("."))
            .await;
        assert!(r.is_error);
        assert!(r.content.contains("not intercepted"), "{}", r.content);
    }
}
