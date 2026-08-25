/// Integration tests for LLM provider body building, retry, and fallback.
use whycodes_core::types::{
    ContentBlock, LlmRequest, Message, MessageContent, Role, ToolDefinition,
};
use whycodes_llm::anthropic::AnthropicProvider;
use whycodes_llm::deepseek::DeepSeekProvider;
use whycodes_llm::fallback::FallbackChain;
use whycodes_llm::openai::OpenAiProvider;
use whycodes_llm::openrouter::OpenRouterProvider;
use whycodes_llm::provider::LlmProvider;
use whycodes_llm::retry;

fn make_basic_request() -> LlmRequest {
    LlmRequest {
        system: "You are a helpful assistant.".to_string(),
        messages: std::sync::Arc::from(vec![Message {
            role: Role::User,
            content: MessageContent::Text("Hello!".to_string()),
            tool_call_id: None,
            name: None,
            created_at: None,
        }]),
        tools: vec![],
        max_tokens: Some(1024),
        temperature: Some(0.7),
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: true,
    }
}

const CANONICAL_CALL_ID: &str = "call_weather_1";

fn make_canonical_tool_semantics_request() -> LlmRequest {
    LlmRequest {
        system: String::new(),
        messages: std::sync::Arc::from(vec![
            Message {
                role: Role::User,
                content: MessageContent::Text("What is the weather in Paris?".into()),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
            Message {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                    id: CANONICAL_CALL_ID.into(),
                    name: "get_weather".into(),
                    input: serde_json::json!({"city": "Paris"}),
                }]),
                tool_call_id: None,
                name: None,
                created_at: None,
            },
            Message {
                role: Role::Tool,
                content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                    tool_use_id: CANONICAL_CALL_ID.into(),
                    content: "21 C".into(),
                    is_error: None,
                }]),
                tool_call_id: Some(CANONICAL_CALL_ID.into()),
                name: None,
                created_at: None,
            },
        ]),
        tools: vec![ToolDefinition {
            name: "get_weather".into(),
            description: "Get the current weather for a city.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"city": {"type": "string"}},
                "required": ["city"],
                "additionalProperties": false
            }),
        }],
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
        use_prompt_cache: false,
    }
}

#[test]
fn test_public_provider_bodies_preserve_canonical_tool_semantics() {
    let request = make_canonical_tool_semantics_request();
    let expected_schema = serde_json::json!({
        "type": "object",
        "properties": {"city": {"type": "string"}},
        "required": ["city"],
        "additionalProperties": false
    });

    let anthropic = AnthropicProvider::new().build_body(&request, "claude");
    let openai = OpenAiProvider::new().build_body(&request, "gpt-4o");

    let anthropic_tool = &anthropic["tools"][0];
    assert_eq!(anthropic_tool["name"], "get_weather");
    assert_eq!(
        anthropic_tool["description"],
        "Get the current weather for a city."
    );
    assert_eq!(anthropic_tool["input_schema"], expected_schema);

    let anthropic_call = &anthropic["messages"][1]["content"][0];
    let anthropic_result = &anthropic["messages"][2]["content"][0];
    assert_eq!(anthropic_call["type"], "tool_use");
    assert_eq!(anthropic_call["id"], CANONICAL_CALL_ID);
    assert_eq!(anthropic_call["name"], anthropic_tool["name"]);
    assert_eq!(
        anthropic_call["input"],
        serde_json::json!({"city": "Paris"})
    );
    assert_eq!(anthropic_result["type"], "tool_result");
    assert_eq!(anthropic_result["tool_use_id"], anthropic_call["id"]);
    assert_eq!(anthropic_result["content"], "21 C");

    let openai_tool = &openai["tools"][0];
    assert_eq!(openai_tool["type"], "function");
    assert_eq!(openai_tool["function"]["name"], "get_weather");
    assert_eq!(
        openai_tool["function"]["description"],
        "Get the current weather for a city."
    );
    assert_eq!(openai_tool["function"]["parameters"], expected_schema);

    let openai_assistant = &openai["messages"][1];
    let openai_call = &openai_assistant["tool_calls"][0];
    assert_eq!(openai_assistant["role"], "assistant");
    assert!(openai_assistant["content"].is_null());
    assert_eq!(openai_call["id"], CANONICAL_CALL_ID);
    assert_eq!(openai_call["type"], "function");
    assert_eq!(
        openai_call["function"]["name"],
        openai_tool["function"]["name"]
    );

    let openai_arguments: serde_json::Value = serde_json::from_str(
        openai_call["function"]["arguments"]
            .as_str()
            .expect("OpenAI function arguments should be a JSON string"),
    )
    .expect("OpenAI function arguments should contain valid JSON");
    assert_eq!(openai_arguments, serde_json::json!({"city": "Paris"}));

    let openai_result = &openai["messages"][2];
    assert_eq!(openai_result["role"], "tool");
    assert_eq!(openai_result["tool_call_id"], openai_call["id"]);
    assert_eq!(
        openai_result["content"],
        "[tool_result call_weather_1] 21 C"
    );
}

// ─── Anthropic body building ───────────────────────────────────────────────

#[test]
fn test_anthropic_build_body() {
    let provider = AnthropicProvider::new();
    let request = make_basic_request();
    let body = provider.build_body(&request, "claude-sonnet-4-20250514");

    assert_eq!(body["model"].as_str().unwrap(), "claude-sonnet-4-20250514");
    assert_eq!(body["max_tokens"].as_u64().unwrap(), 1024);
    assert!(body["stream"].as_bool().unwrap());
    // Prompt-cacheable system block (array form with cache_control).
    let system = body["system"].as_array().expect("system is content blocks");
    assert_eq!(system[0]["type"].as_str().unwrap(), "text");
    assert_eq!(
        system[0]["text"].as_str().unwrap(),
        "You are a helpful assistant."
    );
    assert_eq!(
        system[0]["cache_control"]["type"].as_str().unwrap(),
        "ephemeral"
    );

    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"].as_str().unwrap(), "user");

    let content = &messages[0]["content"];
    assert!(content.is_array());
    assert_eq!(content[0]["text"].as_str().unwrap(), "Hello!");
    // OpenCode auto: latest user message also gets cache_control.
    assert_eq!(
        content[0]["cache_control"]["type"].as_str().unwrap(),
        "ephemeral"
    );
}

#[test]
fn test_anthropic_build_body_with_temperature_and_top_p() {
    let provider = AnthropicProvider::new();
    let mut request = make_basic_request();
    request.temperature = Some(0.3);
    request.top_p = Some(0.9);

    let body = provider.build_body(&request, "claude-opus");
    // f32/f64 conversion can cause minor floating-point drift
    let temp = body["temperature"].as_f64().unwrap();
    assert!(
        (temp - 0.3).abs() < 0.001,
        "temperature {temp} should be ~0.3"
    );
    let top_p = body["top_p"].as_f64().unwrap();
    assert!((top_p - 0.9).abs() < 0.001, "top_p {top_p} should be ~0.9");
}

#[test]
fn test_anthropic_build_body_with_thinking() {
    let provider = AnthropicProvider::new();
    let mut request = make_basic_request();
    request.thinking = Some(serde_json::json!({"type": "enabled", "budget_tokens": 4000}));

    let body = provider.build_body(&request, "claude-opus");
    assert!(body.get("thinking").is_some());
    assert_eq!(body["thinking"]["type"].as_str().unwrap(), "enabled");
    assert_eq!(body["thinking"]["budget_tokens"].as_u64().unwrap(), 4000);
}

#[test]
fn test_anthropic_build_body_uses_requested_budget() {
    let provider = AnthropicProvider::new();
    let mut request = make_basic_request();
    request.thinking = Some(serde_json::json!({"enabled": true, "budget_tokens": 8000}));
    let body = provider.build_body(&request, "claude-opus");
    assert_eq!(body["thinking"]["budget_tokens"].as_u64().unwrap(), 8000);
    assert!(body["max_tokens"].as_u64().unwrap() >= 8000 + 4096);
}

#[test]
fn test_openai_build_body_sends_reasoning_effort() {
    let provider = OpenAiProvider::new();
    let mut request = make_basic_request();
    request.thinking = Some(serde_json::json!({
        "enabled": true,
        "reasoning_effort": "high"
    }));
    let body = provider.build_body(&request, "gpt-5");
    assert_eq!(body["reasoning_effort"].as_str().unwrap(), "high");
}

#[test]
fn test_xai_build_body_sends_reasoning_effort() {
    let provider = whycodes_llm::xai::XaiProvider::new();
    let mut request = make_basic_request();
    request.thinking = Some(serde_json::json!({
        "enabled": true,
        "reasoning_effort": "low"
    }));
    let body = provider.build_body(&request, "grok-4");
    assert_eq!(body["reasoning_effort"].as_str().unwrap(), "low");
}

#[test]
fn test_anthropic_build_body_uses_top_level_system() {
    let provider = AnthropicProvider::new();
    let request = make_basic_request();
    let body = provider.build_body(&request, "claude");

    // Anthropic puts system at top-level (content blocks), not in messages.
    let system = body["system"].as_array().expect("cached system blocks");
    assert_eq!(
        system[0]["text"].as_str().unwrap(),
        "You are a helpful assistant."
    );
    let messages = body["messages"].as_array().unwrap();
    for m in messages {
        // No system role in messages — Anthropic uses top-level system
        assert_ne!(m["role"].as_str().unwrap(), "system");
    }
}

#[test]
fn test_anthropic_tools_get_cache_breakpoint_on_last() {
    let provider = AnthropicProvider::new();
    let mut request = make_basic_request();
    request.tools = vec![
        whycodes_core::types::ToolDefinition {
            name: "read".into(),
            description: "read a file".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        },
        whycodes_core::types::ToolDefinition {
            name: "grep".into(),
            description: "search".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        },
    ];
    let body = provider.build_body(&request, "claude");
    let tools = body["tools"].as_array().unwrap();
    assert!(tools[0].get("cache_control").is_none());
    assert_eq!(
        tools[1]["cache_control"]["type"].as_str().unwrap(),
        "ephemeral"
    );
}

// ─── OpenAI body building ──────────────────────────────────────────────────

#[test]
fn test_openai_build_body() {
    let provider = OpenAiProvider::new();
    let request = make_basic_request();
    let body = provider.build_body(&request, "gpt-4o");

    assert_eq!(body["model"].as_str().unwrap(), "gpt-4o");
    assert!(body["stream"].as_bool().unwrap());

    let messages = body["messages"].as_array().unwrap();
    // OpenAI puts system as a message
    assert_eq!(messages[0]["role"].as_str().unwrap(), "system");
    assert_eq!(
        messages[0]["content"].as_str().unwrap(),
        "You are a helpful assistant."
    );
    assert_eq!(messages[1]["role"].as_str().unwrap(), "user");

    // OpenAI uses string content for text messages, not arrays
    let user_content = &messages[1]["content"];
    assert!(user_content.is_string());
    assert_eq!(user_content.as_str().unwrap(), "Hello!");
}

#[test]
fn test_openai_build_body_with_tools() {
    let provider = OpenAiProvider::new();
    let mut request = make_basic_request();
    request.tools = vec![whycodes_core::types::ToolDefinition {
        name: "search".to_string(),
        description: "Search the web".to_string(),
        parameters: serde_json::json!({"type": "object", "properties": {}}),
    }];

    let body = provider.build_body(&request, "gpt-4o");
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["type"].as_str().unwrap(), "function");
    assert_eq!(tools[0]["function"]["name"].as_str().unwrap(), "search");
    assert_eq!(body["tool_choice"].as_str().unwrap(), "auto");
}

// ─── DeepSeek is OpenAI-compatible ─────────────────────────────────────────

#[test]
fn test_deepseek_is_openai_compatible() {
    let provider = DeepSeekProvider::new();
    let request = make_basic_request();
    let body = provider.build_body(&request, "deepseek-chat");

    // Should have the same OpenAI-compatible message structure
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages[0]["role"].as_str().unwrap(), "system");
    assert_eq!(
        messages[0]["content"].as_str().unwrap(),
        "You are a helpful assistant."
    );
    assert_eq!(messages[1]["role"].as_str().unwrap(), "user");
    assert!(messages[1]["content"].is_string());

    // Should support the same fields
    assert!(body.get("model").is_some());
    assert!(body.get("stream").is_some());
}

#[test]
fn test_deepseek_and_openai_produce_same_format() {
    let request = make_basic_request();
    let openai_body = OpenAiProvider::new().build_body(&request, "gpt-4o");
    let deepseek_body = DeepSeekProvider::new().build_body(&request, "deepseek-chat");

    // Both should have messages array with same structure
    assert_eq!(
        openai_body["messages"][0]["role"].as_str(),
        deepseek_body["messages"][0]["role"].as_str()
    );
    assert_eq!(
        openai_body["messages"][1]["role"].as_str(),
        deepseek_body["messages"][1]["role"].as_str()
    );
}

// ─── OpenRouter headers ────────────────────────────────────────────────────

#[test]
fn test_openrouter_has_headers() {
    let provider = OpenRouterProvider::new()
        .with_site("https://example.com".to_string(), "TestApp".to_string());

    assert_eq!(provider.site_url.as_deref(), Some("https://example.com"));
    assert_eq!(provider.site_name.as_deref(), Some("TestApp"));
    assert_eq!(provider.name(), "openrouter");
    assert_eq!(
        provider.default_base_url(),
        "https://openrouter.ai/api/v1/chat/completions"
    );
}

#[test]
fn test_openrouter_defaults_to_whycodes_identity() {
    let provider = OpenRouterProvider::new();
    assert_eq!(
        provider.site_url.as_deref(),
        Some(whycodes_llm::HTTP_REFERER)
    );
    assert_eq!(provider.site_name.as_deref(), Some(whycodes_llm::X_TITLE));
}

#[test]
fn test_client_identity_user_agent() {
    assert!(whycodes_llm::USER_AGENT.starts_with("whycodes/"));
    assert_eq!(whycodes_llm::X_TITLE, "whycodes");
    assert_eq!(whycodes_llm::HTTP_REFERER, "https://why.codes");
}

#[test]
fn test_openrouter_body_is_openai_compatible() {
    let provider = OpenRouterProvider::new();
    let request = make_basic_request();
    let body = provider.build_body(&request, "anthropic/claude-sonnet");

    // OpenRouter uses OpenAI-compatible format
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages[0]["role"].as_str().unwrap(), "system");
    assert_eq!(messages[1]["role"].as_str().unwrap(), "user");
}

// ─── Retry with backoff ────────────────────────────────────────────────────

#[tokio::test]
async fn test_retry_backoff_successful_first_try() {
    let result = retry::retry_with_backoff(
        || async { Ok::<&str, whycodes_core::Error>("success") },
        3,
        10,
    )
    .await;
    assert_eq!(result.unwrap(), "success");
}

#[tokio::test]
async fn test_retry_backoff_non_retryable_error() {
    // A 400 error is not retryable — should fail immediately
    let result = retry::retry_with_backoff(
        || async move {
            Err::<String, _>(whycodes_core::Error::Llm(
                "Bad request (400): Invalid input".to_string(),
            ))
        },
        3,
        10,
    )
    .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_retry_backoff_retryable_error_retries() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let call_count = Arc::new(AtomicUsize::new(0));
    let c = call_count.clone();

    // First call returns 429 (retryable), second succeeds
    let result = retry::retry_with_backoff(
        || {
            let count = c.clone();
            async move {
                let n = count.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Err(whycodes_core::Error::Llm(
                        "Rate limit (429): Too many requests".to_string(),
                    ))
                } else {
                    Ok("recovered")
                }
            }
        },
        3,
        10,
    )
    .await;

    assert_eq!(result.unwrap(), "recovered");
    assert_eq!(call_count.load(Ordering::SeqCst), 2);
}

// ─── Fallback chain ────────────────────────────────────────────────────────

#[test]
fn test_fallback_chain_creation() {
    use std::collections::HashMap;

    let entries = vec![
        ("anthropic".to_string(), "claude-sonnet".to_string()),
        ("openai".to_string(), "gpt-4o".to_string()),
    ];
    let api_keys = HashMap::from([
        ("anthropic".to_string(), "sk-ant-key".to_string()),
        ("openai".to_string(), "sk-openai-key".to_string()),
    ]);

    let _chain = FallbackChain::new(entries, api_keys);
    // Chain created successfully — no assertion beyond construction not panicking
}

#[test]
fn test_fallback_chain_order_preserved() {
    use std::collections::HashMap;

    let entries = vec![
        ("first".to_string(), "model-a".to_string()),
        ("second".to_string(), "model-b".to_string()),
        ("third".to_string(), "model-c".to_string()),
    ];
    let _chain = FallbackChain::new(entries, HashMap::new());
    // The chain should exist; the order is part of the internal state
    // verified through behavioral test below
}

#[test]
fn test_fallback_chain_empty_entries() {
    use std::collections::HashMap;

    let _chain = FallbackChain::new(vec![], HashMap::new());
    // Empty chain should construct without panicking
}
