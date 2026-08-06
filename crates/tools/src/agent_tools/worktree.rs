use async_trait::async_trait;
use serde_json::json;

use crate::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

/// Manage project-local git worktrees (`.whycode/worktrees/<name>`).
/// Intercepted by the agent loop for enter/exit cwd override.
pub struct WorktreeTool;

impl Default for WorktreeTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WorktreeTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for WorktreeTool {
    fn name(&self) -> &str {
        "worktree"
    }

    fn description(&self) -> &str {
        "Git worktree helpers under `.whycode/worktrees/`. Actions: create, list, \
         remove, enter (switch tool cwd), exit (restore project root). Use to isolate \
         experimental edits without touching the main checkout."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "list", "remove", "enter", "exit"],
                    "description": "create | list | remove | enter | exit"
                },
                "name": {
                    "type": "string",
                    "description": "Worktree name (create/remove/enter)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, _args: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        ToolResult {
            tool_call_id: String::new(),
            content: "worktree was not intercepted by the agent loop.".into(),
            is_error: true,
        }
    }
}
