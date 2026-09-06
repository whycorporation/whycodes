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
fn from_base_blank_keeps_cloud_and_override_normalizes() {
    let cloud = DeepSeekProvider::from_base(Some("   "));
    assert!(cloud.default_base_url().contains("deepseek.com"));
    let local = DeepSeekProvider::from_base(Some("http://127.0.0.1:9/v1"));
    let local_url = local.default_base_url();
    assert!(local_url.ends_with("/chat/completions"), "{local_url}");
    assert_eq!(DeepSeekProvider::default().name(), "deepseek");
}

#[test]
fn build_body_without_tools_applies_sampling_and_effort() {
    let body = DeepSeekProvider::new().build_body(&req(), "deepseek-chat");
    assert!(body.get("tools").is_none());
    assert_eq!(body["reasoning_effort"], "low");
    assert!(body["temperature"].is_number());
    assert!(body["top_p"].is_number());
}
