//! Prewalk: plan on the strong model, then hand the rest of the turn to the
//! cheap one at the first edit.
//!
//! The hand-off reuses the turn's fallback slot (`fallback` in `turn.rs`):
//! the cheap model becomes the active target for the remaining steps, and it
//! stays the starting target of later turns until `/prewalk restart` or a
//! different strong route.

use std::path::Path;

use whycodes_core::types::ToolCall;
use whycodes_llm::provider::LlmProvider;

use super::{Agent, FallbackTarget, recover_lock};

/// Tools that always change files.
const EDIT_TOOLS: [&str; 4] = ["edit", "write", "apply_patch", "ast_edit"];

/// Whether `call` is the first real edit of a plan: a file-editing tool, or a
/// `bash` command that command-risk does not classify as safe.
pub(crate) fn is_mutating_call(call: &ToolCall, working_dir: &Path) -> bool {
    if EDIT_TOOLS.contains(&call.name.as_str()) {
        return true;
    }
    if call.name != "bash" {
        return false;
    }
    let command = call
        .arguments
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    whycodes_command_risk::assess(command, working_dir).level
        != whycodes_command_risk::RiskLevel::Safe
}

/// A strong route and the cheap target it handed off to.
pub(crate) type PrewalkHandoff = (String, String, FallbackTarget);

impl Agent {
    /// `[session] prewalk` (or `--prewalk` / `WHYCODES_PREWALK`) is on.
    pub fn prewalk_enabled(&self) -> bool {
        self.prewalk
    }

    /// `/prewalk restart`: the next turn plans on the strong model again.
    /// `true` when a hand-off was cleared.
    pub fn prewalk_restart(&self) -> bool {
        recover_lock(&self.prewalk_handoff).take().is_some()
    }

    /// The cheap target an earlier turn handed off to from this strong route.
    pub(crate) fn prewalk_resume(
        &self,
        strong_provider: &str,
        strong_model: &str,
    ) -> Option<(FallbackTarget, &dyn LlmProvider)> {
        if !self.prewalk {
            return None;
        }
        let saved = recover_lock(&self.prewalk_handoff).clone()?;
        let (provider, model, target) = saved;
        if provider != strong_provider || model != strong_model {
            return None;
        }
        let next = self.provider_registry.get(&target.provider)?;
        Some((target, next))
    }

    /// The cheap target to switch to once `calls` (already executed) held the
    /// first edit of the turn. `None` keeps the strong model: no edit yet, or
    /// no cheaper sibling with a provider and a key.
    pub(crate) fn prewalk_target(
        &self,
        calls: &[ToolCall],
        working_dir: &str,
        strong_provider: &str,
        strong_model: &str,
        active_key: &str,
    ) -> Option<(FallbackTarget, &dyn LlmProvider)> {
        let dir = Path::new(working_dir);
        if !self.prewalk || !calls.iter().any(|c| is_mutating_call(c, dir)) {
            return None;
        }
        let (provider, model) = crate::routing::resolve_worker_model(
            strong_provider,
            strong_model,
            self.model_smol.as_deref(),
        );
        if provider == strong_provider && model == strong_model {
            tracing::debug!(%provider, %model, "prewalk: no cheaper sibling, staying on the strong model");
            return None;
        }
        let Some(next) = self.provider_registry.get(&provider) else {
            tracing::debug!(%provider, "prewalk: cheap provider not registered, staying");
            return None;
        };
        let Some(api_key) = self.target_key(&provider, strong_provider, active_key) else {
            tracing::debug!(%provider, "prewalk: no key for the cheap provider, staying");
            return None;
        };
        let target = FallbackTarget {
            provider,
            model,
            api_key,
        };
        *recover_lock(&self.prewalk_handoff) = Some((
            strong_provider.to_string(),
            strong_model.to_string(),
            target.clone(),
        ));
        Some((target, next))
    }
}

#[cfg(test)]
#[path = "prewalk_tests.rs"]
mod tests;
