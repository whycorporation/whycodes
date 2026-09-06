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
#[path = "routing_tests.rs"]
mod tests;
