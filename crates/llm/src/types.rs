/// Re-exports of all LLM types for convenience
pub use whycodes_core::types::{
    AgentInfo, AgentMode, ContentBlock, ImageSource, LlmRequest, LlmResponse, Message,
    MessageContent, ModelConfig, PermissionSet, ProviderConfig, Role, SessionInfo, StreamEvent,
    ToolCall, ToolDefinition, ToolResult, Usage,
};

#[cfg(test)]
mod tests {
    #[test]
    fn types_module_loads() {
        assert!(!module_path!().is_empty());
    }
}
