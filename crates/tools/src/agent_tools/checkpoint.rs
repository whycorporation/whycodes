//! Conversation checkpoint / rewind pair (oh-my-pi-style exploratory collapse).
//!
//! The tools themselves are markers. The agent loop records the boundary and
//! later replaces exploratory turns with the rewind report.

use async_trait::async_trait;
use serde_json::json;

use crate::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

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

#[async_trait]
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

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
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

#[async_trait]
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

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn checkpoint_rejects_empty_goal() {
        let t = CheckpointTool::new();
        let r = t
            .execute(json!({"goal": "  "}), &ToolContext::new("/tmp"))
            .await;
        assert!(r.is_error);
    }

    #[tokio::test]
    async fn checkpoint_echoes_goal() {
        let t = CheckpointTool::new();
        let r = t
            .execute(json!({"goal": "find the leak"}), &ToolContext::new("/tmp"))
            .await;
        assert!(!r.is_error);
        assert!(r.content.contains("find the leak"));
        assert_eq!(t.name(), "checkpoint");
        assert!(!t.description().is_empty());
    }

    #[tokio::test]
    async fn rewind_rejects_empty_report() {
        let t = RewindTool::new();
        let r = t
            .execute(json!({"report": ""}), &ToolContext::new("/tmp"))
            .await;
        assert!(r.is_error);
        assert_eq!(t.name(), "rewind");
    }

    #[tokio::test]
    async fn rewind_echoes_report() {
        let t = RewindTool::new();
        let r = t
            .execute(
                json!({"report": "root cause is X"}),
                &ToolContext::new("/tmp"),
            )
            .await;
        assert!(!r.is_error);
        assert!(r.content.contains("root cause is X"));
    }
}
