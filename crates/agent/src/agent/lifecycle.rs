//! Lifecycle hooks around a user turn: `session_start`, `user_prompt_submit`,
//! `stop`, and `pre_compact`. Tool hooks live in `gate.rs`.

use whycodes_config::HookEvent;
use whycodes_core::types::{ContentBlock, MessageContent, Role};
use whycodes_plugin::hooks::{HookContext, run_hooks};
use whycodes_session::session::Session;

use super::{Agent, recover_lock};

impl Agent {
    /// `session_start` (first turn of a session in this process) then
    /// `user_prompt_submit`. Hook feedback is appended to the user turn; a
    /// blocking `user_prompt_submit` failure refuses the turn.
    pub(super) async fn run_prompt_hooks(
        &self,
        session: &mut Session,
        prompt: &str,
    ) -> whycodes_core::Result<()> {
        if self.hooks.is_empty() {
            return Ok(());
        }
        let first = recover_lock(&self.session_starts).insert(session.id.clone());
        if first {
            let ctx = lifecycle_ctx(HookEvent::SessionStart, "", session);
            let outcome = run_hooks(&self.hooks, &ctx).await;
            add_turn_context(session, outcome.feedback_text());
        }
        let ctx = lifecycle_ctx(HookEvent::UserPromptSubmit, "", session)
            .with_env("WHYCODES_PROMPT", prompt);
        let outcome = run_hooks(&self.hooks, &ctx).await;
        if let Some(detail) = outcome.block {
            return Err(whycodes_core::Error::Agent(format!(
                "user_prompt_submit hook blocked this prompt: {detail}"
            )));
        }
        add_turn_context(session, outcome.feedback_text());
        Ok(())
    }

    /// `stop` after a turn produced its reply. Never blocks. `before` is the
    /// session usage when the turn started, so the hook also gets this
    /// turn's token counts next to the session totals.
    pub(super) async fn run_stop_hooks(
        &self,
        session: &Session,
        reply: &str,
        before: &whycodes_core::types::Usage,
    ) {
        if self.hooks.is_empty() {
            return;
        }
        let total = &session.usage;
        let turn_in = total.input_tokens.saturating_sub(before.input_tokens);
        let turn_out = total.output_tokens.saturating_sub(before.output_tokens);
        let ctx = lifecycle_ctx(HookEvent::Stop, "", session)
            .with_env("WHYCODES_RESPONSE", reply)
            .with_env("WHYCODES_TURN_INPUT_TOKENS", &turn_in.to_string())
            .with_env("WHYCODES_TURN_OUTPUT_TOKENS", &turn_out.to_string())
            .with_env(
                "WHYCODES_SESSION_INPUT_TOKENS",
                &total.input_tokens.to_string(),
            )
            .with_env(
                "WHYCODES_SESSION_OUTPUT_TOKENS",
                &total.output_tokens.to_string(),
            );
        run_hooks(&self.hooks, &ctx).await;
    }

    /// `pre_compact` before the transcript is replaced by a summary.
    pub(super) async fn run_pre_compact_hooks(&self, session: &Session) {
        if self.hooks.is_empty() {
            return;
        }
        let ctx = lifecycle_ctx(HookEvent::PreCompact, "", session);
        run_hooks(&self.hooks, &ctx).await;
    }
}

fn lifecycle_ctx(event: HookEvent, subject: &str, session: &Session) -> HookContext {
    HookContext::lifecycle(
        event,
        subject,
        Some(session.id.clone()),
        session.project_path.display().to_string(),
    )
}

/// Append hook output to the latest user message so the model sees it.
fn add_turn_context(session: &mut Session, feedback: Option<String>) {
    let Some(text) = feedback else {
        return;
    };
    let Some(msg) = session
        .messages
        .iter_mut()
        .rev()
        .find(|m| m.role == Role::User)
    else {
        return;
    };
    let note = format!("\n\n<hook-context>\n{text}\n</hook-context>");
    match &mut msg.content {
        MessageContent::Text(body) => body.push_str(&note),
        MessageContent::Blocks(blocks) => blocks.push(ContentBlock::Text { text: note }),
    }
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
