use async_trait::async_trait;
use serde_json::json;

use crate::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

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

#[async_trait]
impl Tool for SwarmTool {
    fn name(&self) -> &str {
        "swarm"
    }

    fn description(&self) -> &str {
        "Run several subagents in parallel on independent units of work. \
         Each worker claims files it writes; a second worker that touches the \
         same path is blocked and the user is notified (conflict notify). \
         Prefer for wide audits or mechanical migrations across disjoint paths. \
         For a single investigation use `task` instead."
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

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let n = args["tasks"].as_array().map(|a| a.len()).unwrap_or(0);
        ToolResult {
            tool_call_id: String::new(),
            content: format!(
                "Swarm tool was not intercepted by the agent loop ({} tasks).",
                n
            ),
            is_error: true,
        }
    }
}
