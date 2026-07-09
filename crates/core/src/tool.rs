use async_trait::async_trait;

use crate::types::{PermissionSet, ToolDefinition, ToolResult};

/// Context passed to tool execution
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub working_dir: String,
    pub session_id: Option<String>,
}

/// A tool that can be invoked by the LLM
#[async_trait]
pub trait Tool: Send + Sync {
    /// Name of the tool
    fn name(&self) -> &str;

    /// Description for the LLM
    fn description(&self) -> &str;

    /// JSON Schema for the tool parameters
    fn parameters(&self) -> serde_json::Value;

    /// Execute the tool
    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult;

    /// Get the full tool definition for LLM requests
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters(),
        }
    }

    /// Whether this tool is allowed given a permission set.
    ///
    /// Checks in priority order:
    /// 1. If the tool name is in `denied_tools`, deny.
    /// 2. If `allowed_tools` is set and the tool is NOT in it, deny.
    /// 3. For write-type tools (write, edit, apply_patch, todo_write):
    ///    deny if `allow_file_writes` is false.
    /// 4. For `shell`: deny if `allow_shell` is false.
    /// 5. For network tools (webfetch, websearch): deny if `allow_network` is false.
    /// 6. Otherwise allow.
    fn is_allowed(&self, permissions: &PermissionSet) -> bool {
        let name = self.name().to_string();

        // 1. Explicit deny list
        if let Some(denied) = &permissions.denied_tools {
            if denied.contains(&name) {
                return false;
            }
        }

        // 2. Explicit allow list (if set, only those are allowed)
        if let Some(allowed) = &permissions.allowed_tools {
            if !allowed.contains(&name) {
                return false;
            }
        }

        // 3. Write-type tools: check allow_file_writes
        if matches!(
            self.name(),
            "write" | "edit" | "apply_patch" | "todo_write" | "todowrite" | "todo"
        ) {
            if !permissions.allow_file_writes {
                return false;
            }
        }

        // 4. Shell: check allow_shell
        if self.name() == "shell" && !permissions.allow_shell {
            return false;
        }

        // 5. Network tools: check allow_network
        if matches!(self.name(), "webfetch" | "websearch") && !permissions.allow_network {
            return false;
        }

        true
    }
}
