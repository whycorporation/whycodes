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
    let raw = match fast_override {
        Some(raw) => raw.trim(),
        None => "",
    };
    if !raw.is_empty() {
        match raw.split_once('/') {
            Some((p, m)) => return (p.to_string(), m.to_string()),
            None => return (provider.to_string(), raw.to_string()),
        }
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
    let raw = match plan_override {
        Some(raw) => raw.trim(),
        None => "",
    };
    if agent_name.eq_ignore_ascii_case("plan") && !raw.is_empty() {
        return resolve_title_model(provider, model, Some(raw));
    }
    (provider.to_string(), model.to_string())
}

#[cfg(test)]
#[path = "routing_tests.rs"]
mod tests;
