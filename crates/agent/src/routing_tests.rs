use super::*;

#[test]
fn coding_prompt_keeps_main_model() {
    let (p, m) = resolve_turn_model(
        "anthropic",
        "claude-sonnet-4-5",
        "fix the auth bug in session.rs",
        Some("anthropic/claude-haiku-4-5-20251001"),
    );
    assert_eq!(p, "anthropic");
    assert_eq!(m, "claude-sonnet-4-5");
}

#[test]
fn greeting_uses_fast_override() {
    let (p, m) = resolve_turn_model(
        "anthropic",
        "claude-sonnet-4-5",
        "selam",
        Some("anthropic/claude-haiku-4-5-20251001"),
    );
    assert_eq!(p, "anthropic");
    assert!(m.contains("haiku"));
}

#[test]
fn greeting_without_override_picks_sibling() {
    let (p, m) = resolve_turn_model("openai", "gpt-4o", "hi", None);
    assert_eq!(p, "openai");
    assert!(m.contains("mini") || m == "gpt-4o-mini");
}

#[test]
fn worker_model_uses_smol_override_or_sibling() {
    let (p, m) = resolve_worker_model(
        "anthropic",
        "claude-sonnet-4-5",
        Some("anthropic/claude-haiku-4-5-20251001"),
    );
    assert_eq!(p, "anthropic");
    assert!(m.contains("haiku"));
    let (p, m) = resolve_worker_model("openai", "gpt-4o", None);
    assert_eq!(p, "openai");
    assert!(m.contains("mini"));
}

#[test]
fn plan_agent_uses_override_others_keep_session_model() {
    let (p, m) = resolve_agent_model(
        "anthropic",
        "claude-sonnet-4-5",
        "plan",
        Some("anthropic/claude-opus-4-6"),
    );
    assert_eq!(p, "anthropic");
    assert!(m.contains("opus"));
    let (p, m) = resolve_agent_model("anthropic", "claude-sonnet-4-5", "build", Some("opus"));
    assert_eq!(p, "anthropic");
    assert_eq!(m, "claude-sonnet-4-5");
}

#[test]
fn override_bare_id_keeps_provider() {
    let (p, m) = resolve_override("xai", "grok-4", Some("grok-3-mini"));
    assert_eq!(p, "xai");
    assert_eq!(m, "grok-3-mini");
}

#[test]
fn greeting_bare_fast_override_keeps_provider() {
    let (p, m) = resolve_turn_model("anthropic", "claude-sonnet-4-5", "hi", Some("haiku"));
    assert_eq!(p, "anthropic");
    assert_eq!(m, "haiku");
}
