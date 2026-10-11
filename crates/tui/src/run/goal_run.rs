//! `/goal` in the run loop: count each finished turn, ask the evaluator off
//! the UI thread, and act on its verdict (continue, achieved, impossible).
use super::*;
use crate::goal::{TurnStep, Verdict};

/// One evaluator answer: verdict and reason, or why the check failed.
pub(crate) type GoalCheck = Result<(Verdict, String), String>;

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
    match goal.record_turn(used_tools) {
        TurnStep::Pause => {
            let why = goal.paused.clone().unwrap_or_default();
            app.add_message(
                ChatRole::System,
                format!("◎ goal paused: {why}. {GOAL_RESUME_HINT}"),
            );
        }
        TurnStep::Evaluate => {
            let request =
                crate::goal::evaluator_request(&goal.condition, goal.turns, &rt.session.messages);
            let (provider, model) = whycodes_agent::resolve_title_model(
                provider,
                model,
                config.session.model_fast.as_deref(),
            );
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
    match goal_transport()
        .complete_uncached(prov, request, api_key, model)
        .await
    {
        Ok(response) => {
            let text = response
                .content
                .iter()
                .filter_map(|b| match b {
                    whycodes_core::types::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            Ok(crate::goal::parse_verdict(&text))
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Overloads and dropped connections retry; a dead provider pauses the goal.
fn goal_transport() -> whycodes_llm::LlmTransport {
    whycodes_llm::LlmTransport {
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
/// turn unless the user already has a prompt waiting.
pub(super) fn apply_goal_check(app: &mut TuiApp, rt: &mut SessionRuntime, check: GoalCheck) {
    let Some(goal) = rt.goal.as_mut() else {
        // Cleared while the check was in flight.
        return;
    };
    goal.evaluating = false;
    app.mark_dirty();
    let (verdict, reason) = match check {
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
    let (line, toast) = match verdict {
        Verdict::NotMet => {
            app.add_message(ChatRole::System, format!("◎ goal not met yet: {reason}"));
            let next = crate::goal::continue_prompt(&goal.condition, &reason);
            goal.last_reason = Some(reason);
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
            Ok((Verdict::NotMet, reason)) => {
                goal.last_reason = Some(reason);
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
