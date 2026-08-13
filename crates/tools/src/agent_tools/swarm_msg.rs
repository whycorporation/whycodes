//! Send a DM or broadcast on the swarm mailbox.

use async_trait::async_trait;
use serde_json::json;

use crate::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

/// Message another swarm worker, or the parent, or everyone.
pub struct SwarmMsgTool;

impl Default for SwarmMsgTool {
    fn default() -> Self {
        Self::new()
    }
}

impl SwarmMsgTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for SwarmMsgTool {
    fn name(&self) -> &str {
        "swarm_msg"
    }

    fn description(&self) -> &str {
        "Send a short message to another swarm worker (`to` = worker-N), the \
         parent agent (`parent`), or every sibling (`all`). Use when your work \
         affects someone else. Not a chat — one or two sentences."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "to": {
                    "type": "string",
                    "description": "Recipient: parent | all | worker-N"
                },
                "text": {
                    "type": "string",
                    "description": "Message body"
                }
            },
            "required": ["to", "text"]
        })
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let Some(hub) = ctx.swarm_hub.as_ref() else {
            return ToolResult {
                tool_call_id: String::new(),
                content: "swarm_msg is only available inside a swarm run.".into(),
                is_error: true,
            };
        };
        let to = args.get("to").and_then(|v| v.as_str()).unwrap_or("").trim();
        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if to.is_empty() || text.is_empty() {
            return ToolResult {
                tool_call_id: String::new(),
                content: "swarm_msg requires `to` and `text`.".into(),
                is_error: true,
            };
        }
        let from = ctx
            .agent_id
            .as_deref()
            .or(ctx.agent_label.as_deref())
            .unwrap_or("worker");
        hub.send(from, to, text);
        ToolResult {
            tool_call_id: String::new(),
            content: format!("Sent to {to}."),
            is_error: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whycode_core::SwarmHub;

    #[tokio::test]
    async fn send_reaches_inbox() {
        let hub = SwarmHub::new();
        let mut ctx = ToolContext::unsandboxed(".");
        ctx.swarm_hub = Some(hub.clone());
        ctx.agent_id = Some("worker-0".into());
        let result = SwarmMsgTool::new()
            .execute(
                json!({"to": "worker-1", "text": "file a.rs is yours"}),
                &ctx,
            )
            .await;
        assert!(!result.is_error, "{}", result.content);
        let got = hub.drain("worker-1");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].from, "worker-0");
        assert_eq!(got[0].text, "file a.rs is yours");
    }
}
