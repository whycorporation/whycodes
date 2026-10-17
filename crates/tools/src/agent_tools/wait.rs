use serde_json::json;

use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

/// Default `timeout_secs` when the model omits it.
pub const WAIT_DEFAULT_SECS: u64 = 60;
/// Upper bound for `timeout_secs`; larger values are clamped.
pub const WAIT_MAX_SECS: u64 = 300;

/// Block until a background job finishes, a user message arrives, or the
/// timeout elapses. Intercepted by the agent loop.
pub struct WaitTool;

impl Default for WaitTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WaitTool {
    pub fn new() -> Self {
        Self
    }
}

/// `timeout_secs` from the call arguments: default 60, clamped to 300.
pub fn wait_timeout_secs(args: &serde_json::Value) -> u64 {
    args.get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(WAIT_DEFAULT_SECS)
        .min(WAIT_MAX_SECS)
}

/// Trimmed, non-empty `job_id` from the call arguments.
pub fn wait_job_id(args: &serde_json::Value) -> Option<&str> {
    args.get("job_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|id| !id.is_empty())
}

impl Tool for WaitTool {
    fn name(&self) -> &str {
        "wait"
    }

    fn description(&self) -> &str {
        "Block until something you are waiting for happens instead of polling `bg` every step: \
         a background job finishes (its id, exit status and output tail come back), a message \
         from the user arrives (reported as pending; end the step to read it), or `timeout_secs` \
         elapses. Returns at once when no job is running."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "timeout_secs": {
                    "type": "integer",
                    "description": "Seconds to wait before giving up (default 60, max 300)"
                },
                "job_id": {
                    "type": "string",
                    "description": "Only wait for this background job (e.g. bg-1); default: any job"
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
                content: "wait tool was not intercepted by the agent loop.".into(),
                is_error: true,
            }
        })
    }
}

#[cfg(test)]
#[path = "wait_tests.rs"]
mod tests;
