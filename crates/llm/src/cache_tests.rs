use super::*;

#[test]
fn auto_marks_system_tools_and_latest_user() {
    let mut body = serde_json::json!({
        "system": "You are WhyCodes.",
        "tools": [
            {"name": "read", "description": "r", "input_schema": {"type": "object"}},
            {"name": "grep", "description": "g", "input_schema": {"type": "object"}},
        ],
        "messages": [
            {"role": "user", "content": [{"type": "text", "text": "first"}]},
            {"role": "assistant", "content": [{"type": "text", "text": "ok"}]},
            {"role": "user", "content": [{"type": "text", "text": "latest"}]},
        ]
    });
    apply_anthropic_cache_policy(&mut body, &CacheConfig::default());

    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
    assert!(body["tools"][0].get("cache_control").is_none());
    assert_eq!(body["tools"][1]["cache_control"]["type"], "ephemeral");
    assert!(
        body["messages"][0]["content"][0]
            .get("cache_control")
            .is_none()
    );
    assert_eq!(
        body["messages"][2]["content"][0]["cache_control"]["type"],
        "ephemeral"
    );
}

#[test]
fn none_policy_leaves_body() {
    let mut body = serde_json::json!({
        "system": "x",
        "tools": [{"name": "a", "input_schema": {}}],
        "messages": [{"role": "user", "content": [{"type": "text", "text": "hi"}]}],
    });
    apply_anthropic_cache_policy(&mut body, &CacheConfig::disabled());
    assert_eq!(body["system"], "x");
    assert!(body["tools"][0].get("cache_control").is_none());
}

#[test]
fn parse_policy() {
    assert_eq!(CachePolicy::parse("auto"), CachePolicy::Auto);
    assert_eq!(CachePolicy::parse("none"), CachePolicy::None);
    assert_eq!(CachePolicy::parse("off"), CachePolicy::None);
}

#[test]
fn from_policy_and_serde_defaults() {
    assert_eq!(
        CacheConfig::from_policy(CachePolicy::Auto),
        CacheConfig::default()
    );
    assert_eq!(
        CacheConfig::from_policy(CachePolicy::None),
        CacheConfig::disabled()
    );
    let parsed: CacheConfig = serde_json::from_str("{}").unwrap();
    assert!(parsed.system && parsed.tools && parsed.messages == 1);
    assert_eq!(CacheConfig::cache_control_value()["type"], "ephemeral");
}

#[test]
fn system_array_and_null_and_string_user_content() {
    let mut body = serde_json::json!({
        "system": [{"type": "text", "text": "sys"}],
        "tools": [{"name": "a", "input_schema": {}}],
        "messages": [{"role": "user", "content": "plain"}]
    });
    apply_anthropic_cache_policy(&mut body, &CacheConfig::default());
    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
    assert_eq!(
        body["messages"][0]["content"][0]["cache_control"]["type"],
        "ephemeral"
    );

    let mut body = serde_json::json!({
        "system": null,
        "messages": [{"role": "assistant", "content": [{"type": "text", "text": "x"}]}]
    });
    apply_anthropic_cache_policy(&mut body, &CacheConfig::default());
    assert!(body["system"].is_null());

    let mut body = serde_json::json!({
        "messages": [{"role": "user", "content": []}]
    });
    apply_anthropic_cache_policy(
        &mut body,
        &CacheConfig {
            system: false,
            tools: false,
            messages: 1,
        },
    );
    assert!(
        body["messages"][0]["content"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let mut body = serde_json::json!({"system": "x"});
    apply_anthropic_cache_policy(&mut body, &CacheConfig::default());
    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
    assert!(body.get("messages").is_none());
}
