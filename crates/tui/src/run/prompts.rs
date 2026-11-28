//! Permission prompts and questionnaires: the overlays, their keys and the
//! replies sent back to the waiting tool.
use super::*;

/// Overlay keys that answer a permission prompt (allow / deny). Other keys
/// fall through to the normal dialog handler.
pub(super) fn permission_overlay_reply(code: KeyCode) -> Option<bool> {
    match code {
        KeyCode::Char('y' | 'Y' | 'a' | 'A') | KeyCode::Enter => Some(true),
        KeyCode::Char('n' | 'N' | 'd' | 'D') | KeyCode::Esc => Some(false),
        _ => None,
    }
}

/// Permission overlay Y/N/A/D/Enter/Esc, and Tab to type a reply instead.
/// Returns true when the event is consumed.
pub(super) fn apply_permission_overlay_event(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    ev: &Event,
) -> bool {
    if !matches!(app.dialogs.active(), Some(DialogKind::Permission { .. })) {
        return false;
    }
    if app.permission_reply_focus {
        return apply_permission_reply_event(app, rt, ev);
    }
    let Event::Key(key) = ev else {
        return false;
    };
    if key.kind != KeyEventKind::Press {
        return false;
    }
    if key.code == KeyCode::Tab {
        app.permission_reply_focus = true;
        app.focus_prompt();
        app.mark_dirty();
        return true;
    }
    let Some(allow) = permission_overlay_reply(key.code) else {
        return false;
    };
    reply_permission(app, &mut rt.pending_perm_queue, allow);
    true
}

/// Prompt box focused over the permission prompt. Keys and pastes edit the
/// draft; Tab / Esc go back to the y/n keys; a plain Enter denies the
/// pending asks and sends the draft to the running turn. Mouse and resize
/// events keep their usual path.
pub(super) fn apply_permission_reply_event(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    ev: &Event,
) -> bool {
    match ev {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Tab | KeyCode::Esc => {
                app.permission_reply_focus = false;
                app.mark_dirty();
            }
            KeyCode::Enter if key.modifiers.is_empty() => reply_instead_of_permission(app, rt),
            _ => type_into_prompt(app, ev),
        },
        Event::Key(_) | Event::Paste(_) => type_into_prompt(app, ev),
        _ => return false,
    }
    true
}

/// Run one event through the normal prompt handler while the permission
/// prompt stays open, then hand the keys back to the dialog stack.
pub(super) fn type_into_prompt(app: &mut TuiApp, ev: &Event) {
    app.mode = AppMode::Normal;
    app.key_context = KeymapContext::Normal;
    let keep_running = input::handle_event(app, ev.clone());
    app.running &= keep_running;
    if app.dialogs.is_open() {
        app.mode = AppMode::Dialog;
        app.key_context = KeymapContext::Dialog;
    }
}

/// Deny every pending ask with "user replied" and queue the draft; the run
/// loop hands it to the turn, which reads it after the denied results.
/// An empty draft or a slash command is not a reply.
pub(super) fn reply_instead_of_permission(app: &mut TuiApp, rt: &mut SessionRuntime) {
    let queued = app.pending_turns.len();
    app.submit_input();
    if app.pending_turns.len() == queued {
        app.toasts.push(
            crate::toast::ToastKind::Info,
            "Type a message to send, or Tab back to allow / deny",
        );
        app.mark_dirty();
        return;
    }
    let denied = rt.pending_perm_queue.len();
    while let Some(req) = rt.pending_perm_queue.pop_front() {
        if req.reply.send(PermissionReply::Replied).is_err() {
            tracing::debug!(tool = %req.tool_name, "permission waiter already gone");
        }
    }
    app.permission_reply_focus = false;
    app.dialogs.pop();
    if !app.dialogs.is_open() {
        app.mode = AppMode::Normal;
        app.key_context = KeymapContext::Normal;
    }
    app.current_agent_state = AgentState::Generating;
    app.status_message = format!("Denied {denied} tool call(s) — your message goes to the turn");
    app.mark_dirty();
}

/// Question overlay keys (Press and Repeat). Returns true when consumed.
pub(super) fn apply_question_overlay_event(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    ev: &Event,
) -> bool {
    if !matches!(app.dialogs.active(), Some(DialogKind::Question(_))) {
        return false;
    }
    let Event::Key(key) = ev else {
        return false;
    };
    if key.kind != KeyEventKind::Press && key.kind != KeyEventKind::Repeat {
        return false;
    }
    let handled = handle_question_key(
        app,
        key.code,
        &mut rt.pending_question_queue,
        &rt.pending_perm_queue,
    );
    if handled {
        app.mark_dirty();
    }
    handled
}

/// Unblock every tool still waiting on the user: questions are cancelled and
/// permission prompts denied, so the agent can observe the cancel promptly.
/// A send fails only when the turn already dropped its receiver.
pub(super) fn reject_pending(
    question_queue: &mut std::collections::VecDeque<QuestionRequest>,
    perm_queue: &mut std::collections::VecDeque<whycodes_agent::PermissionRequest>,
) {
    while let Some(req) = question_queue.pop_front() {
        let _ = req.reply.send(Err(QuestionError::Cancelled));
    }
    while let Some(req) = perm_queue.pop_front() {
        let _ = req.reply.send(PermissionReply::Deny);
    }
}

pub(super) fn reply_permission(
    app: &mut TuiApp,
    queue: &mut std::collections::VecDeque<whycodes_agent::PermissionRequest>,
    allow: bool,
) {
    if let Some(req) = queue.pop_front() {
        let _ = req.reply.send(allow.into());
    }
    app.dialogs.pop();
    app.mode = AppMode::Normal;
    app.key_context = KeymapContext::Normal;
    if let Some(next) = queue.front() {
        app.ask_permission(next.tool_name.clone(), next.detail.clone());
        app.status_message = format!(
            "{} — {} more permission(s)…",
            if allow { "Allowed" } else { "Denied" },
            queue.len()
        );
    } else if allow {
        app.current_agent_state = AgentState::Generating;
        app.status_message = "Allowed — continuing…".into();
    } else {
        app.current_agent_state = AgentState::Generating;
        app.status_message = "Denied tool".into();
    }
}

pub(super) fn complete_questionnaire_ui(
    app: &mut TuiApp,
    queue: &mut std::collections::VecDeque<QuestionRequest>,
    perm_queue: &std::collections::VecDeque<whycodes_agent::PermissionRequest>,
    answers: Option<Vec<whycodes_tools::question::QuestionAnswer>>,
) {
    if let Some(req) = queue.pop_front() {
        let _ = match answers {
            Some(a) => req.reply.send(Ok(a)),
            None => req.reply.send(Err(QuestionError::Cancelled)),
        };
    }
    if matches!(app.dialogs.active(), Some(DialogKind::Question(_))) {
        app.dialogs.pop();
        app.clear_dialog_hits();
    }
    if !matches!(app.dialogs.active(), Some(DialogKind::Question(_))) {
        app.mode = AppMode::Normal;
        app.key_context = KeymapContext::Normal;
        resume_after_question(app, queue, perm_queue);
    }
}

/// Complete a questionnaire oneshot set by mouse / `[✗]` in `input.rs`.
///
/// Must run **after** `handle_event` (those flags are written there) and on
/// idle ticks so a click is not stuck until the next keypress (issue #41).
pub(super) fn flush_pending_question_replies(
    app: &mut TuiApp,
    queue: &mut std::collections::VecDeque<QuestionRequest>,
    perm_queue: &std::collections::VecDeque<whycodes_agent::PermissionRequest>,
) {
    if let Some(answers) = app.pending_question_answers.take() {
        complete_questionnaire_ui(app, queue, perm_queue, Some(answers));
    }
    if app.question_dismissed {
        app.question_dismissed = false;
        complete_questionnaire_ui(app, queue, perm_queue, None);
    }
}

pub(super) fn maybe_open_queued_dialog(app: &mut TuiApp, rt: &SessionRuntime) {
    // Key off the actual overlay, not a stale WaitingFor* state. A dismissed
    // question can leave WaitingForQuestion with an empty stack (issue #41).
    if matches!(
        app.dialogs.active(),
        Some(DialogKind::Permission { .. } | DialogKind::Question(_))
    ) {
        return;
    }
    if let Some(front) = rt.pending_perm_queue.front() {
        app.ask_permission(front.tool_name.clone(), front.detail.clone());
        app.mark_dirty();
    } else if let Some(front) = rt.pending_question_queue.front() {
        app.ask_question(front.questions.clone());
        app.mark_dirty();
    }
}

pub(super) fn notify_permission(req: &whycodes_agent::PermissionRequest) {
    crate::desktop_notify::notify(
        whycodes_config::NotifyEvent::NeedInput,
        &crate::desktop_notify::permission_body(&req.tool_name),
    );
}

pub(super) fn notify_question(req: &QuestionRequest) {
    let first = req.questions.first().map(|q| q.prompt.as_str());
    crate::desktop_notify::notify(
        whycodes_config::NotifyEvent::NeedInput,
        &crate::desktop_notify::question_body(first),
    );
}

/// Move newly arrived prompter requests onto the runtime queues, then open one overlay.
pub(super) fn drain_prompter_queues(app: &mut TuiApp, rt: &mut SessionRuntime) {
    while let Ok(req) = rt.perm_rx.try_recv() {
        notify_permission(&req);
        rt.pending_perm_queue.push_back(req);
    }
    while let Ok(req) = rt.question_rx.try_recv() {
        notify_question(&req);
        rt.pending_question_queue.push_back(req);
    }
    maybe_open_queued_dialog(app, rt);
}

/// Handle keys while a questionnaire panel is open. Returns true if consumed.
pub(super) fn handle_question_key(
    app: &mut TuiApp,
    code: KeyCode,
    pending_question_queue: &mut std::collections::VecDeque<QuestionRequest>,
    pending_perm_queue: &std::collections::VecDeque<whycodes_agent::PermissionRequest>,
) -> bool {
    let Some(DialogKind::Question(mut state)) = app.dialogs.pop() else {
        return false;
    };

    let finish_cancel = |app: &mut TuiApp,
                         pending_question_queue: &mut std::collections::VecDeque<
        QuestionRequest,
    >,
                         pending_perm_queue: &std::collections::VecDeque<
        whycodes_agent::PermissionRequest,
    >| {
        if let Some(req) = pending_question_queue.pop_front() {
            let _ = req.reply.send(Err(QuestionError::Cancelled));
        }
        app.mode = AppMode::Normal;
        app.key_context = KeymapContext::Normal;
        app.clear_dialog_hits();
        resume_after_question(app, pending_question_queue, pending_perm_queue);
    };

    let finish_ok = |app: &mut TuiApp,
                     answers: Vec<whycodes_tools::question::QuestionAnswer>,
                     pending_question_queue: &mut std::collections::VecDeque<QuestionRequest>,
                     pending_perm_queue: &std::collections::VecDeque<
        whycodes_agent::PermissionRequest,
    >| {
        if let Some(req) = pending_question_queue.pop_front() {
            let _ = req.reply.send(Ok(answers));
        }
        app.mode = AppMode::Normal;
        app.key_context = KeymapContext::Normal;
        app.clear_dialog_hits();
        resume_after_question(app, pending_question_queue, pending_perm_queue);
    };

    match code {
        KeyCode::Esc => {
            // Mid-edit Other with text: first Esc leaves the field.
            // Empty free-text (including option-less questions) cancels immediately.
            if state.free_text_focus && !state.free_text.is_empty() {
                state.free_text_focus = false;
                app.dialogs.push(DialogKind::Question(state));
                return true;
            }
            finish_cancel(app, pending_question_queue, pending_perm_queue);
            true
        }
        KeyCode::Up | KeyCode::Char('k') if !state.free_text_focus => {
            state.move_cursor(-1);
            app.dialogs.push(DialogKind::Question(state));
            true
        }
        KeyCode::Down | KeyCode::Char('j') if !state.free_text_focus => {
            state.move_cursor(1);
            app.dialogs.push(DialogKind::Question(state));
            true
        }
        // Multi-question navigate (←/→ between questions)
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('[') if !state.free_text_focus => {
            state.go_prev_question();
            app.dialogs.push(DialogKind::Question(state));
            true
        }
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(']') if !state.free_text_focus => {
            state.go_next_question();
            app.dialogs.push(DialogKind::Question(state));
            true
        }
        // Copy full questionnaire to clipboard
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('c') | KeyCode::Char('C')
            if !state.free_text_focus =>
        {
            let text = state.clipboard_text();
            if crate::clipboard::copy_text(&text) {
                app.toasts.push(
                    crate::toast::ToastKind::Info,
                    format!("Copied question ({} chars)", text.chars().count()),
                );
            } else {
                app.toasts.push(
                    crate::toast::ToastKind::Warning,
                    "Copy failed — no clipboard",
                );
            }
            app.dialogs.push(DialogKind::Question(state));
            true
        }
        KeyCode::Char(' ') if !state.free_text_focus => {
            if state.current().map(|q| q.multi_select).unwrap_or(false) {
                state.toggle_multi_at_cursor();
            } else if state.is_other_index(state.cursor) {
                state.free_text_focus = true;
            }
            app.dialogs.push(DialogKind::Question(state));
            true
        }
        KeyCode::Char('o') | KeyCode::Char('O') if !state.free_text_focus => {
            // Jump to Other…
            let other = state.option_count().saturating_sub(1);
            state.cursor = other;
            state.free_text_focus = true;
            app.dialogs.push(DialogKind::Question(state));
            true
        }
        KeyCode::Enter => {
            if let Some(answers) = state.confirm_current() {
                finish_ok(app, answers, pending_question_queue, pending_perm_queue);
            } else {
                // Still on this question (e.g. empty Other → focus free text)
                app.dialogs.push(DialogKind::Question(state));
            }
            true
        }
        KeyCode::Backspace if state.free_text_focus => {
            state.free_text.pop();
            app.dialogs.push(DialogKind::Question(state));
            true
        }
        KeyCode::Char(c) if state.free_text_focus && !c.is_control() => {
            state.free_text.push(c);
            app.dialogs.push(DialogKind::Question(state));
            true
        }
        KeyCode::Char(c) if !state.free_text_focus && !c.is_control() => {
            // Digit shortcut 1..n for single-select
            if let Some(d) = c.to_digit(10) {
                let idx = (d as usize).saturating_sub(1);
                if idx < state.option_count() {
                    state.cursor = idx;
                    if state.is_other_index(idx) {
                        state.free_text_focus = true;
                        app.dialogs.push(DialogKind::Question(state));
                    } else if state.current().map(|q| q.multi_select).unwrap_or(false) {
                        state.multi_selected.insert(idx);
                        app.dialogs.push(DialogKind::Question(state));
                    } else if let Some(answers) = state.confirm_current() {
                        finish_ok(app, answers, pending_question_queue, pending_perm_queue);
                    } else {
                        app.dialogs.push(DialogKind::Question(state));
                    }
                    return true;
                }
            }
            app.dialogs.push(DialogKind::Question(state));
            false
        }
        _ => {
            app.dialogs.push(DialogKind::Question(state));
            false
        }
    }
}

pub(super) fn resume_after_question(
    app: &mut TuiApp,
    pending_question_queue: &std::collections::VecDeque<QuestionRequest>,
    pending_perm_queue: &std::collections::VecDeque<whycodes_agent::PermissionRequest>,
) {
    if let Some(next) = pending_question_queue.front() {
        app.ask_question(next.questions.clone());
        app.status_message = format!(
            "Answered — {} more question set(s)…",
            pending_question_queue.len()
        );
    } else if let Some(next) = pending_perm_queue.front() {
        app.ask_permission(next.tool_name.clone(), next.detail.clone());
    } else {
        app.current_agent_state = AgentState::Generating;
        app.status_message = "Answered — continuing…".into();
    }
}
