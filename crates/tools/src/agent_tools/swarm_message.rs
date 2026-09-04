//! Send a DM or broadcast on the swarm mailbox.

use serde_json::json;

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

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

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whycodes_core::SwarmHub;

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

    #[tokio::test]
    async fn missing_hub_and_args() {
        let t = SwarmMsgTool;
        assert_eq!(t.name(), "swarm_msg");
        assert!(!t.description().is_empty());
        assert_eq!(t.parameters()["required"], json!(["to", "text"]));
        let no_hub = t
            .execute(
                json!({"to": "all", "text": "hi"}),
                &ToolContext::unsandboxed("."),
            )
            .await;
        assert!(no_hub.is_error);
        assert!(
            no_hub.content.contains("only available"),
            "{}",
            no_hub.content
        );

        let mut ctx = ToolContext::unsandboxed(".");
        ctx.swarm_hub = Some(SwarmHub::new());
        ctx.agent_label = Some("parent".into());
        let missing = t.execute(json!({"to": " ", "text": ""}), &ctx).await;
        assert!(missing.is_error);
        let ok = t.execute(json!({"to": "all", "text": "hello"}), &ctx).await;
        assert!(!ok.is_error, "{}", ok.content);
    }
}
