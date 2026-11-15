//! `[security.decision]`: a decision-model second opinion on shell commands
//! that `command-risk` allowed. It can only add a prompt; a failed or slow
//! call keeps the classifier's decision. See
//! `docs/proposals/decision-models.md`.

use whycodes_config::DecisionConfig;
use whycodes_core::types::{ApprovalMode, Message, PermissionAction, Role};
use whycodes_llm::decision::DecisionClient;
use whycodes_session::session::Session;

use super::Agent;

impl Agent {
    /// Ask the decision model about a shell `command` that `command-risk`
    /// allowed. `Some(reason)` means prompt with that reason.
    pub(super) async fn decision_escalation(
        &self,
        command: &str,
        session: &Session,
    ) -> Option<String> {
        let gate = self.decision.as_ref()?;
        // `auto` never prompts, so the answer could not change anything.
        if self.approval_mode == ApprovalMode::Auto && !self.fail_closed_ask {
            return None;
        }
        // The user already allowed this command shape explicitly.
        if self.info.permission.action_for_shell(command) == Some(PermissionAction::Allow) {
            return None;
        }
        let redact = |text: &str| match &self.secrets {
            Some(redactor) => redactor
                .redact_text(text)
                .unwrap_or_else(|| text.to_string()),
            None => text.to_string(),
        };
        let request = last_human_request(&session.messages);
        gate.escalation(&redact(command), &redact(&request)).await
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DecisionGate {
    client: DecisionClient,
    threshold: f64,
}

impl DecisionGate {
    /// `None` unless `[security.decision] enabled = true`.
    pub(crate) fn from_config(config: &DecisionConfig) -> Option<Self> {
        if !config.enabled {
            return None;
        }
        let api_key = if config.api_key_env.is_empty() {
            None
        } else {
            std::env::var(&config.api_key_env).ok()
        };
        Some(Self {
            client: DecisionClient::new(config, api_key),
            threshold: config.threshold,
        })
    }

    /// `Some(reason)` when the score reaches the threshold. `None` below it,
    /// and when the call fails (logged; the caller keeps its decision).
    pub(crate) async fn escalation(&self, command: &str, user_request: &str) -> Option<String> {
        match self.client.shell_verdict(command, user_request).await {
            Ok(verdict) if verdict.risk >= self.threshold => {
                Some(verdict.reason(self.client.model()))
            }
            Ok(verdict) => {
                tracing::debug!(risk = verdict.risk, "decision model: below threshold");
                None
            }
            Err(error) => {
                tracing::warn!(%error, "decision model unavailable; keeping the classifier's decision");
                None
            }
        }
    }
}

/// Notices whycodes stores with the user role. They are not what the user
/// asked for, and sending them as the request degrades the fit question.
/// New notes carry [`Message::is_agent_note`]; these prefixes cover sessions
/// written before that tag and notices queued from outside the agent.
const NOTICE_PREFIXES: &[&str] = &[
    "The tool failed with error:",
    "Background job `",
    "Continue working toward the",
    "You stopped while your todo",
    "This session is being continued",
];

fn is_notice(text: &str) -> bool {
    NOTICE_PREFIXES.iter().any(|p| text.starts_with(p))
        || (text.starts_with('`') && text.contains("` failed; its output is in the tool result"))
}

/// The last message the user wrote, skipping notices stored with the user
/// role. Empty when there is none.
pub(crate) fn last_human_request(messages: &[Message]) -> String {
    messages
        .iter()
        .rev()
        .filter(|m| m.role == Role::User && !m.is_agent_note())
        .filter_map(|m| m.content.as_text())
        .map(str::trim)
        .find(|text| !text.is_empty() && !is_notice(text))
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
#[path = "decision_gate_tests.rs"]
mod tests;
