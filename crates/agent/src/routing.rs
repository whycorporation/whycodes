//! Fast/default model routing for latency.
//!
//! Trivial chit-chat ("selam", "ok", "ping") does not need a flagship model.
//! When `model_fast` is configured — or when we can pick a known small sibling
//! — route those turns to the cheap model. Real coding prompts stay on the
//! session model.

use crate::title::{is_trivial_title_seed, resolve_title_model};

/// Decide provider+model for this user message.
///
/// - Non-trivial prompts always keep `(provider, model)`.
/// - Trivial prompts prefer `fast_override` (`provider/model` or bare id), else
///   the same small-sibling logic as session title refine.
pub fn resolve_turn_model(
    provider: &str,
    model: &str,
    user_text: &str,
    fast_override: Option<&str>,
) -> (String, String) {
    if !is_trivial_title_seed(user_text) {
        return (provider.to_string(), model.to_string());
    }
    // Explicit override wins (config session.model_fast).
    if let Some(raw) = fast_override.map(str::trim).filter(|s| !s.is_empty()) {
        if let Some((p, m)) = raw.split_once('/') {
            return (p.to_string(), m.to_string());
        }
        return (provider.to_string(), raw.to_string());
    }
    // Reuse small-model sibling table (haiku/mini/flash/…).
    resolve_title_model(provider, model, None)
}

/// Parse `provider/model` or a bare model id (same provider).
pub fn resolve_override(
    provider: &str,
    model: &str,
    override_model: Option<&str>,
) -> (String, String) {
    resolve_title_model(provider, model, override_model)
}

/// Cheap model for `task` / `swarm` workers.
pub fn resolve_worker_model(
    provider: &str,
    model: &str,
    smol_override: Option<&str>,
) -> (String, String) {
    resolve_title_model(provider, model, smol_override)
}

/// Plan-agent model when `model_plan` is set; otherwise keep the session model.
pub fn resolve_agent_model(
    provider: &str,
    model: &str,
    agent_name: &str,
    plan_override: Option<&str>,
) -> (String, String) {
    if agent_name.eq_ignore_ascii_case("plan")
        && let Some(raw) = plan_override.map(str::trim).filter(|s| !s.is_empty())
    {
        return resolve_title_model(provider, model, Some(raw));
    }
    (provider.to_string(), model.to_string())
}

#[cfg(test)]
mod tests {
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
}
