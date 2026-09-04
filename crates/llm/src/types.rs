/// Re-exports of all LLM types for convenience
pub use whycodes_core::types::{
    AgentInfo, AgentMode, ContentBlock, ImageSource, LlmRequest, LlmResponse, Message,
    MessageContent, ModelConfig, PermissionSet, ProviderConfig, Role, SessionInfo, StreamEvent,
    ToolCall, ToolDefinition, ToolResult, Usage,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reexports_core_llm_types() {
        let _ = Role::User;
        let _ = Role::Assistant;
        let req = LlmRequest {
            system: "s".into(),
            messages: std::sync::Arc::from([]),
            tools: std::sync::Arc::from([]),
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            thinking: None,
            use_prompt_cache: false,
        };
        assert!(req.system == "s");
        let _ = StreamEvent::MessageStop;
    }
}
