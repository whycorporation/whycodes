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
}
