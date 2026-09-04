use serde_json::json;

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

/// Parallel multi-agent work with file-ownership conflict notify.
///
/// Real execution is intercepted by `Agent::run_turn` (needs provider/model).
pub struct SwarmTool;

impl Default for SwarmTool {
    fn default() -> Self {
        Self::new()
    }
}

impl SwarmTool {
    pub fn new() -> Self {
        Self
    }
}
impl Tool for SwarmTool {
    fn name(&self) -> &str {
        "swarm"
    }

    fn description(&self) -> &str {
        "Run several subagents in parallel on independent units of work. \
         In a git repo each worker gets an isolated worktree under `.whycodes/swarm/`; \
         changes three-way-merge back into the main checkout (conflicts toast). \
         Without git (or with worktrees off), workers share the checkout and file \
         claims block double-writes. Prefer for wide audits or mechanical migrations \
         across disjoint paths. For a single investigation use `task` instead."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "tasks": {
                    "type": "array",
                    "description": "Parallel units of work (2–8 items)",
                    "items": {
                        "type": "object",
                        "properties": {
                            "goal": {
                                "type": "string",
                                "description": "Goal for this worker"
                            },
                            "context": {
                                "type": "string",
                                "description": "Optional extra context for this worker"
                            },
                            "subagent_type": {
                                "type": "string",
                                "enum": ["general", "explore", "scout"],
                                "description": "Worker type (default: general)"
                            },
                            "paths": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Optional paths to pre-claim before the worker starts (reduces mid-turn conflicts)"
                            },
                            "max_turns": {
                                "type": "integer",
                                "description": "Max conversation turns for this worker (default: 15)"
                            }
                        },
                        "required": ["goal"]
                    },
                    "minItems": 1
                },
                "max_concurrent": {
                    "type": "integer",
                    "description": "Max workers running at once (default from config, usually 4)"
                }
            },
            "required": ["tasks"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        _ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            let n = args["tasks"].as_array().map(|a| a.len()).unwrap_or(0);
            ToolResult {
                tool_call_id: String::new(),
                content: format!(
                    "Swarm tool was not intercepted by the agent loop ({} tasks).",
                    n
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
    fn swarm_module_loads() {
        assert!(!module_path!().is_empty());
    }

    #[tokio::test]
    async fn execute_reports_task_count() {
        let t = SwarmTool;
        assert_eq!(t.name(), "swarm");
        assert!(!t.description().is_empty());
        let params = t.parameters();
        assert_eq!(params["required"][0], "tasks");
        let r = t
            .execute(
                serde_json::json!({"tasks": [{"goal": "a"}, {"goal": "b"}]}),
                &ToolContext::new("."),
            )
            .await;
        assert!(r.is_error);
        assert!(r.content.contains("2 tasks"), "{}", r.content);
        let empty = t
            .execute(serde_json::json!({}), &ToolContext::new("."))
            .await;
        assert!(empty.content.contains("0 tasks"), "{}", empty.content);
    }
}
