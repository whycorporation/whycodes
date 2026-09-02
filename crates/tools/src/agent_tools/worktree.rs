use serde_json::json;

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

/// Manage project-local git worktrees (`.whycodes/worktrees/<name>`).
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
impl Tool for WorktreeTool {
    fn name(&self) -> &str {
        "worktree"
    }

    fn description(&self) -> &str {
        "Git worktree helpers under `.whycodes/worktrees/`. Actions: create, list, \
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

    fn execute<'a>(
        &'a self,
        _args: serde_json::Value,
        _ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            ToolResult {
                tool_call_id: String::new(),
                content: "worktree was not intercepted by the agent loop.".into(),
                is_error: true,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn worktree_module_loads() {
        assert!(!module_path!().is_empty());
    }
}
