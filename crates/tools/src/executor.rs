use std::collections::HashMap;

use whycode_core::types::{PermissionSet, ToolCall, ToolResult};

use super::tool::{Tool, ToolContext};
use crate::{edit, github_issue, github_pr, glob, grep, read, shell, webfetch, websearch, write};

/// Central executor that manages all available tools
pub struct ToolExecutor {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolExecutor {
    /// Create a new executor with all built-in tools
    pub fn new() -> Self {
        let mut executor = Self {
            tools: HashMap::new(),
        };

        executor.register(Box::new(read::ReadTool::new()));
        executor.register(Box::new(write::WriteTool::new()));
        executor.register(Box::new(edit::EditTool::new()));
        executor.register(Box::new(grep::GrepTool::new()));
        executor.register(Box::new(glob::GlobTool::new()));
        executor.register(Box::new(shell::ShellTool::new()));
        executor.register(Box::new(webfetch::WebFetchTool::new()));
        executor.register(Box::new(websearch::WebSearchTool::new()));
        executor.register(Box::new(github_issue::GithubIssueTool::new()));
        executor.register(Box::new(github_pr::GitHubPrTool::new()));

        executor
    }

    /// Register a tool
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        self.tools.insert(name, tool);
    }

    /// Get a tool by name
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    /// Get all tool definitions for LLM requests, filtered by permissions
    pub fn get_definitions(&self, permissions: &PermissionSet) -> Vec<whycode_core::types::ToolDefinition> {
        self.tools
            .values()
            .filter(|t| t.is_allowed(permissions))
            .map(|t| t.definition())
            .collect()
    }

    /// Execute a single tool call
    pub async fn execute(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        permissions: &PermissionSet,
    ) -> ToolResult {
        match self.get(&call.name) {
            Some(tool) => {
                if !tool.is_allowed(permissions) {
                    ToolResult {
                        tool_call_id: call.id.clone(),
                        content: format!("Tool '{}' is not allowed with current permissions.", call.name),
                        is_error: true,
                    }
                } else {
                    tool.execute(call.arguments.clone(), ctx).await
                }
            }
            None => ToolResult {
                tool_call_id: call.id.clone(),
                content: format!(
                    "Unknown tool: '{}'. Available tools: {}",
                    call.name,
                    self.tools
                        .keys()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                is_error: true,
            },
        }
    }
}

impl Default for ToolExecutor {
    fn default() -> Self {
        Self::new()
    }
}
