//! Conversation checkpoint / rewind pair (oh-my-pi-style exploratory collapse).
//!
//! The tools themselves are markers. The agent loop records the boundary and
//! later replaces exploratory turns with the rewind report.

use serde_json::json;

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

pub struct CheckpointTool;

impl Default for CheckpointTool {
    fn default() -> Self {
        Self
    }
}

impl CheckpointTool {
    pub fn new() -> Self {
        Self
    }
}
impl Tool for CheckpointTool {
    fn name(&self) -> &str {
        "checkpoint"
    }

    fn description(&self) -> &str {
        "Mark the current conversation so a later `rewind` can collapse \
         exploratory context into a short report. Use before a speculative \
         investigation. Does not snapshot files."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "goal": {
                    "type": "string",
                    "description": "What you are about to investigate"
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
            let goal = args
                .get("goal")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if goal.is_empty() {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: "checkpoint requires a non-empty `goal`".into(),
                    is_error: true,
                };
            }
            ToolResult {
                tool_call_id: String::new(),
                content: format!(
                    "Checkpoint created.\nGoal: {goal}\n\
                 Run your investigation, then call rewind with a concise report."
                ),
                is_error: false,
            }
        })
    }
}

pub struct RewindTool;

impl Default for RewindTool {
    fn default() -> Self {
        Self
    }
}

impl RewindTool {
    pub fn new() -> Self {
        Self
    }
}
impl Tool for RewindTool {
    fn name(&self) -> &str {
        "rewind"
    }

    fn description(&self) -> &str {
        "End an active `checkpoint` by dropping exploratory conversation \
         after the mark and keeping `report`. Files are not reverted."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "report": {
                    "type": "string",
                    "description": "Concise findings to keep after collapsing the investigation"
                }
            },
            "required": ["report"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        _ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            let report = args
                .get("report")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if report.is_empty() {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: "rewind requires a non-empty `report`".into(),
                    is_error: true,
                };
            }
            ToolResult {
                tool_call_id: String::new(),
                content: format!(
                    "Rewind requested.\nReport captured for context replacement.\n\n{report}"
                ),
                is_error: false,
            }
        })
    }
}

#[cfg(test)]
#[path = "checkpoint_tests.rs"]
mod tests;
