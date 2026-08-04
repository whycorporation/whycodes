use async_trait::async_trait;

use crate::config::SandboxSettings;
use crate::network::NetworkPolicy;
use crate::types::{PermissionSet, ToolDefinition, ToolResult};

/// Context passed to tool execution
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub working_dir: String,
    pub session_id: Option<String>,
    /// OS sandbox policy for shell tools (ignored by pure file tools).
    pub sandbox: SandboxSettings,
    /// Domain allow/deny for HTTP tools (`webfetch`, `websearch`, GitHub API).
    pub network: NetworkPolicy,
}

impl ToolContext {
    pub fn new(working_dir: impl Into<String>) -> Self {
        Self {
            working_dir: working_dir.into(),
            session_id: None,
            sandbox: SandboxSettings::default(),
            network: NetworkPolicy::unrestricted(),
        }
    }

    pub fn unsandboxed(working_dir: impl Into<String>) -> Self {
        Self {
            working_dir: working_dir.into(),
            session_id: None,
            sandbox: SandboxSettings::off(),
            network: NetworkPolicy::unrestricted(),
        }
    }
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
    /// Uses OpenCode-style resolution: `rules` (allow/ask/deny), then legacy
    /// allowed/denied lists and category flags. `Ask` still returns true so the
    /// tool appears in the schema; the agent loop prompts before execution.
    fn is_allowed(&self, permissions: &PermissionSet) -> bool {
        permissions.is_tool_allowed(self.name())
    }
}
