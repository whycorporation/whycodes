use super::*;
use crate::provider::LlmProvider;
use whycodes_core::types::{LlmRequest, Message, MessageContent, Role};

fn req() -> LlmRequest {
    LlmRequest {
        system: "sys".into(),
        messages: std::sync::Arc::from(vec![Message {
            role: Role::User,
            content: MessageContent::Text("hi".into()),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]),
        tools: std::sync::Arc::from([]),
        max_tokens: None,
        temperature: Some(0.5),
        top_p: Some(0.9),
        top_k: None,
        stop_sequences: None,
        thinking: Some(serde_json::json!({"enabled": true, "reasoning_effort": "low"})),
        use_prompt_cache: false,
    }
}

#[test]
fn from_base_blank_keeps_cloud_and_site_override() {
    let cloud = OpenRouterProvider::from_base(Some("   "));
    assert!(cloud.default_base_url().contains("openrouter.ai"));
    let local = OpenRouterProvider::from_base(Some("http://127.0.0.1:9/v1"))
        .with_site("https://example.test".into(), "WhyCodes".into());
    let local_url = local.default_base_url();
    assert!(local_url.ends_with("/chat/completions"), "{local_url}");
    assert_eq!(local.site_url.as_deref(), Some("https://example.test"));
    assert_eq!(OpenRouterProvider::default().name(), "openrouter");
}

#[test]
fn build_body_without_tools_applies_sampling_and_effort() {
    let body = OpenRouterProvider::new().build_body(&req(), "openrouter/auto");
    assert!(body.get("tools").is_none());
    assert_eq!(body["reasoning_effort"], "low");
    assert!(body["temperature"].is_number());
    assert!(body["top_p"].is_number());

    let mut with_tools = req();
    with_tools.tools = vec![whycodes_core::types::ToolDefinition {
        name: "read".into(),
        description: "read a file".into(),
        parameters: serde_json::json!({"type": "object"}),
    }]
    .into();
    let body = OpenRouterProvider::new().build_body(&with_tools, "openrouter/auto");
    assert!(body["tools"].is_array());
    assert_eq!(body["tool_choice"], "auto");
    assert_eq!(body["parallel_tool_calls"], true);
}
