//! `/goal` in the run loop: count each finished turn, ask the evaluator off
//! the UI thread, and act on its verdict (continue, achieved, impossible).
use super::*;
use crate::goal::{TurnStep, Verdict};
use whycodes_core::types::Role;

/// One evaluator answer, or why the check failed.
pub(crate) type GoalCheck = Result<GoalAnswer, String>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GoalAnswer {
    pub verdict: Verdict,
    pub reason: String,
    /// Input plus output tokens the check spent (both attempts).
    pub tokens: u64,
}

/// Asked again once when the first answer has no verdict line.
const VERDICT_REMINDER: &str = "Your answer had no verdict line. Reply with exactly two lines \
and no markdown:\nVERDICT: met | not_met | impossible\nREASON: one sentence";

const GOAL_RESUME_HINT: &str = "Send a prompt to resume.";

/// After a successful turn: pause on too many idle turns, else ask the
/// evaluator. The answer arrives on this runtime's `goal_rx`.
pub(super) fn goal_after_turn(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    config: &Config,
    provider: &str,
    model: &str,
    api_key: &str,
) {
    let Some(goal) = rt.goal.as_mut() else {
        return;
    };
    let used_tools = crate::goal::turn_used_tools(&rt.session.messages);
    let step = goal.record_turn(used_tools);
    crate::goal::save(rt.db.as_ref(), &rt.session.id, Some(goal));
    match step {
        TurnStep::Pause => {
            let why = goal.paused.clone().unwrap_or_default();
            app.add_message(
                ChatRole::System,
                format!("◎ goal paused: {why}. {GOAL_RESUME_HINT}"),
            );
        }
        TurnStep::Evaluate => {
            let request = crate::goal::evaluator_request(
                &goal.condition,
                goal.turns,
                &goal.findings,
                &rt.session.messages,
            );
            let (provider, model) =
                crate::goal::checker_model(provider, model, config.session.goal_model.as_deref());
            let mut registry = whycodes_llm::provider::ProviderRegistry::default();
            registry.register_from_config(config);
            let api_key = api_key.to_string();
            let tx = rt.goal_tx.clone();
            tokio::spawn(async move {
                let check = run_goal_check(&registry, &provider, &request, &api_key, &model).await;
                if tx.send(check).is_err() {
                    tracing::debug!("goal check dropped: session runtime closed");
                }
            });
        }
    }
}

/// A failed or cancelled turn pauses the goal instead of looping on it.
pub(super) fn pause_goal_after_error(app: &mut TuiApp, rt: &mut SessionRuntime, cancelled: bool) {
    let Some(goal) = rt.goal.as_mut() else {
        return;
    };
    if goal.paused.is_some() {
        return;
    }
    let why = if cancelled {
        "turn cancelled"
    } else {
        "turn failed"
    };
    goal.pause(why);
    app.add_message(
        ChatRole::System,
        format!("◎ goal paused: {why}. {GOAL_RESUME_HINT}"),
    );
}

pub(super) async fn run_goal_check(
    registry: &whycodes_llm::provider::ProviderRegistry,
    provider: &str,
    request: &whycodes_core::types::LlmRequest,
    api_key: &str,
    model: &str,
) -> GoalCheck {
    let Some(prov) = registry.get(provider) else {
        return Err(format!("provider {provider} is not configured"));
    };
    let transport = goal_transport();
    let mut request = request.clone();
    let mut tokens = 0;
    let mut text = String::new();
    for attempt in 0..2 {
        if attempt == 1 {
            // Same question, plus the unreadable answer and a reminder of
            // the format: at temperature 0 a bare retry repeats itself.
            let mut messages = request.messages.to_vec();
            messages.push(goal_message(Role::Assistant, &text));
            messages.push(goal_message(Role::User, VERDICT_REMINDER));
            request.messages = std::sync::Arc::from(messages);
        }
        let response = transport
            .complete_uncached(prov, &request, api_key, model)
            .await
            .map_err(|e| e.to_string())?;
        tokens += response.usage.input_tokens + response.usage.output_tokens;
        text = response
            .content
            .iter()
            .filter_map(|b| match b {
                whycodes_core::types::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if let (Some(verdict), reason) = crate::goal::parse_verdict(&text) {
            return Ok(GoalAnswer {
                verdict,
                reason,
                tokens,
            });
        }
    }
    let snippet: String = text.trim().chars().take(120).collect();
    Err(format!("the check gave no verdict ({snippet:?})"))
}

fn goal_message(role: Role, text: &str) -> whycodes_core::types::Message {
    whycodes_core::types::Message {
        role,
        content: whycodes_core::types::MessageContent::Text(text.to_string()),
        tool_call_id: None,
        name: None,
        created_at: None,
    }
}

/// Overloads and dropped connections retry; a dead provider pauses the goal.
fn goal_transport() -> whycodes_llm::LlmTransport {
    whycodes_llm::LlmTransport {
        redactor: None,
        complete_timeout: Some(std::time::Duration::from_secs(60)),
        retry: whycodes_llm::RetryPolicy {
            max_retries: 3,
            initial_backoff: std::time::Duration::from_millis(500),
            max_backoff: std::time::Duration::from_secs(8),
            max_elapsed: std::time::Duration::from_secs(90),
            full_jitter: true,
        },
    }
}

/// Visible session: met / impossible clear the goal; not met queues the next
/// turn unless the user already has a prompt waiting, the goal was paused
/// while the check ran (Esc, a failed turn), or this run reached
/// `max_turns` (`session.goal_max_turns`, 0 = no cap).
pub(super) fn apply_goal_check(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    check: GoalCheck,
    max_turns: u32,
) {
    let Some(goal) = rt.goal.as_mut() else {
        // Cleared while the check was in flight.
        return;
    };
    goal.evaluating = false;
    app.mark_dirty();
    let GoalAnswer {
        verdict,
        reason,
        tokens,
    } = match check {
        Ok(answer) => answer,
        Err(e) => {
            goal.pause(format!("goal check failed: {e}"));
            app.add_message(
                ChatRole::System,
                format!("◎ goal paused: goal check failed: {e}. {GOAL_RESUME_HINT}"),
            );
            return;
        }
    };
    goal.check_tokens += tokens;
    let (line, toast) = match verdict {
        Verdict::NotMet => {
            goal.record_finding(&reason);
            if goal.paused.is_some() {
                app.add_message(ChatRole::System, format!("◎ goal not met yet: {reason}"));
                return;
            }
            if max_turns > 0 && goal.turns_this_run >= max_turns {
                goal.pause(format!(
                    "not met after {max_turns} turns (session.goal_max_turns)"
                ));
                app.add_message(
                    ChatRole::System,
                    format!(
                        "◎ goal paused: not met after {max_turns} turns — {reason}. {GOAL_RESUME_HINT}"
                    ),
                );
                return;
            }
            app.add_message(ChatRole::System, format!("◎ goal not met yet: {reason}"));
            let next = crate::goal::continue_prompt(&goal.condition, &reason);
            if !rt.agent_busy && !app.has_pending_turn() {
                app.enqueue_prompt_text(next);
            }
            return;
        }
        Verdict::Met => (
            format!("◎ goal achieved: {reason}"),
            crate::toast::ToastKind::Success,
        ),
        Verdict::Impossible => (
            format!("◎ goal stopped, not achievable: {reason}"),
            crate::toast::ToastKind::Warning,
        ),
    };
    rt.goal = None;
    crate::goal::save(rt.db.as_ref(), &rt.session.id, None);
    app.add_message(ChatRole::System, &line);
    app.toasts.push(toast, line);
}

/// Background session: nothing to queue into, so not met pauses until the
/// user switches back and prompts. Met / impossible still clear the goal.
pub(super) fn drain_background_goal(rt: &mut SessionRuntime) {
    while let Ok(check) = rt.goal_rx.try_recv() {
        let Some(goal) = rt.goal.as_mut() else {
            continue;
        };
        goal.evaluating = false;
        rt.unread = true;
        match check {
            Ok(GoalAnswer {
                verdict: Verdict::NotMet,
                reason,
                tokens,
            }) => {
                goal.check_tokens += tokens;
                goal.record_finding(&reason);
                goal.pause("session was in the background");
            }
            Ok(_) => {
                rt.goal = None;
                crate::goal::save(rt.db.as_ref(), &rt.session.id, None);
            }
            Err(e) => goal.pause(format!("goal check failed: {e}")),
        }
    }
}
