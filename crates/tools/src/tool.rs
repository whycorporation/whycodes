use async_trait::async_trait;

use whycode_core::types::{PermissionSet, ToolDefinition, ToolResult};

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

    /// Whether this tool is allowed given a permission set
    fn is_allowed(&self, permissions: &PermissionSet) -> bool {
        if let Some(denied) = &permissions.denied_tools {
            if denied.contains(&self.name().to_string()) {
                return false;
            }
        }
        if let Some(allowed) = &permissions.allowed_tools {
            return allowed.contains(&self.name().to_string());
        }
        true
    }
}
