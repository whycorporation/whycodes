use super::*;

#[test]
fn test_default() {
    let cfg = ThinkingConfig::default();
    assert!(cfg.enabled);
    assert_eq!(cfg.budget_tokens, 4000);
}

#[test]
fn test_to_anthropic_enabled() {
    let cfg = ThinkingConfig::new(2000);
    let v = cfg.to_anthropic_value();
    assert_eq!(v["type"], "enabled");
    assert_eq!(v["budget_tokens"], 2000);
}

#[test]
fn test_to_anthropic_disabled() {
    let cfg = ThinkingConfig::disabled();
    let v = cfg.to_anthropic_value();
    assert_eq!(v["type"], "disabled");
}

#[test]
fn test_from_legacy_bool() {
    let v = serde_json::json!(true);
    let cfg = ThinkingConfig::from_request_value(Some(&v)).unwrap();
    assert!(cfg.enabled);
    assert_eq!(cfg.budget_tokens, 4000);
}

#[test]
fn test_from_structured() {
    let v = serde_json::json!({"enabled": true, "budget_tokens": 8000});
    let cfg = ThinkingConfig::from_request_value(Some(&v)).unwrap();
    assert!(cfg.enabled);
    assert_eq!(cfg.budget_tokens, 8000);
}

#[test]
fn from_structured_reads_reasoning_effort() {
    let v = serde_json::json!({"enabled": true, "reasoning_effort": "high"});
    let cfg = ThinkingConfig::from_request_value(Some(&v)).unwrap();
    assert_eq!(cfg.reasoning_effort.as_deref(), Some("high"));
    let mut body = serde_json::json!({});
    ThinkingConfig::apply_openai_effort(&mut body, Some(&v));
    assert_eq!(body["reasoning_effort"], "high");
}

#[test]
fn type_disabled_disables_thinking() {
    let v = serde_json::json!({"type": "disabled"});
    let cfg = ThinkingConfig::from_request_value(Some(&v)).unwrap();
    assert!(!cfg.enabled);
}

#[test]
fn apply_anthropic_raises_max_tokens_above_budget() {
    let v = serde_json::json!({"enabled": true, "budget_tokens": 8000});
    let mut body = serde_json::json!({"max_tokens": 1024});
    ThinkingConfig::apply_anthropic(&mut body, Some(&v));
    assert_eq!(body["thinking"]["budget_tokens"], 8000);
    assert_eq!(body["max_tokens"], 8000 + 4096);
    let disabled = serde_json::json!({"enabled": false, "budget_tokens": 8000});
    let mut keep = serde_json::json!({"max_tokens": 1024});
    ThinkingConfig::apply_anthropic(&mut keep, Some(&disabled));
    assert_eq!(keep["max_tokens"], 1024);
}

#[test]
fn default_effort_for_grok_and_openai() {
    assert_eq!(
        ThinkingConfig::default_effort("xai", "grok-4"),
        Some("medium")
    );
    assert_eq!(
        ThinkingConfig::default_effort("openai", "gpt-5"),
        Some("medium")
    );
    assert_eq!(
        ThinkingConfig::default_effort("anthropic", "claude-sonnet-4"),
        None
    );
}

#[test]
fn from_structured_reads_xhigh() {
    let v = serde_json::json!({"enabled": true, "reasoning_effort": "xhigh"});
    let cfg = ThinkingConfig::from_request_value(Some(&v)).unwrap();
    assert_eq!(cfg.reasoning_effort.as_deref(), Some("xhigh"));
}

#[test]
fn grok_46_offers_xhigh_older_does_not() {
    assert!(ReasoningEffort::model_allows_xhigh("xai", "grok-4.6"));
    assert!(ReasoningEffort::model_allows_xhigh("xai", "grok-4.6-beta"));
    assert!(!ReasoningEffort::model_allows_xhigh("xai", "grok-4"));
    assert!(!ReasoningEffort::model_allows_xhigh("xai", "grok-4.5"));
    assert_eq!(
        ThinkingConfig::supported_efforts("xai", "grok-4.6").len(),
        4
    );
    assert_eq!(ThinkingConfig::supported_efforts("xai", "grok-4").len(), 3);
    assert_eq!(
        ThinkingConfig::resolve_effort("xai", "grok-4", Some("xhigh")),
        Some(ReasoningEffort::High)
    );
    assert_eq!(
        ThinkingConfig::resolve_effort("xai", "grok-4.6", Some("max")),
        Some(ReasoningEffort::XHigh)
    );
    assert!(ThinkingConfig::supported_efforts("anthropic", "claude-sonnet-4").is_empty());
}

#[test]
fn apply_openai_effort_skips_none_and_disabled() {
    let mut body = serde_json::json!({});
    ThinkingConfig::apply_openai_effort(&mut body, None);
    assert!(body.get("reasoning_effort").is_none());
    let v = serde_json::json!({"type": "disabled", "reasoning_effort": "high"});
    ThinkingConfig::apply_openai_effort(&mut body, Some(&v));
    assert!(body.get("reasoning_effort").is_none());
    let enabled_no_effort = serde_json::json!({"enabled": true, "budget_tokens": 100});
    ThinkingConfig::apply_openai_effort(&mut body, Some(&enabled_no_effort));
    assert!(body.get("reasoning_effort").is_none());
}

#[test]
fn reasoning_effort_parse_labels_and_unknown() {
    assert_eq!(
        ReasoningEffort::parse("minimal"),
        Some(ReasoningEffort::Low)
    );
    assert_eq!(ReasoningEffort::parse("MED"), Some(ReasoningEffort::Medium));
    assert_eq!(
        ReasoningEffort::parse("ultra"),
        Some(ReasoningEffort::XHigh)
    );
    assert!(ReasoningEffort::parse("nope").is_none());
    assert_eq!(ReasoningEffort::Low.as_str(), "low");
    assert_eq!(ReasoningEffort::Medium.as_str(), "medium");
    assert_eq!(ReasoningEffort::Medium.label(), "Med");
    assert_eq!(ReasoningEffort::Low.label(), "Low");
    assert_eq!(ReasoningEffort::High.label(), "High");
    assert_eq!(ReasoningEffort::XHigh.label(), "Max");
    assert!(!ReasoningEffort::Low.description().is_empty());
    assert!(!ReasoningEffort::Medium.description().is_empty());
    assert!(!ReasoningEffort::High.description().is_empty());
    assert!(!ReasoningEffort::XHigh.description().is_empty());
    assert_eq!(ReasoningEffort::ALL.len(), 4);
    assert!(!ReasoningEffort::model_allows_xhigh("openai", "gpt-5"));
    assert!(!grok_version_at_least("gpt-5", 4, 6));
    let cfg = ThinkingConfig::from_request_value(Some(&serde_json::json!(false))).unwrap();
    assert!(!cfg.enabled);
    let cfg = ThinkingConfig::from_request_value(Some(&serde_json::json!({
        "enabled": true,
        "budget_tokens": 0
    })))
    .unwrap();
    assert_eq!(cfg.budget_tokens, 4000);
    let mut body = serde_json::json!({"max_tokens": 20_000});
    ThinkingConfig::apply_anthropic(
        &mut body,
        Some(&serde_json::json!({"enabled": true, "budget_tokens": 100})),
    );
    assert_eq!(body["max_tokens"], 20_000);
    let mut body = serde_json::json!({"max_tokens": 9000});
    ThinkingConfig::apply_anthropic(
        &mut body,
        Some(&serde_json::json!({"enabled": true, "budget_tokens": 4000})),
    );
    assert_eq!(body["max_tokens"], 9000);
    let parsed: ThinkingConfig = serde_json::from_str(r#"{"budget_tokens":1}"#).unwrap();
    assert!(parsed.enabled);
    assert_eq!(parsed.budget_tokens, 1);
    assert!(ThinkingConfig::resolve_effort("anthropic", "claude-sonnet-4", None).is_none());
}
