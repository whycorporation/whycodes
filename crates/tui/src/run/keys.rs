//! Key routing while idle and while a turn runs: which layer owns a key and
//! what it does.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum IdleLoopKey {
    CycleAgent,
    NewSession,
    SessionLimit,
    CycleSession {
        next: bool,
    },
    Dashboard,
    MruSwitch,
    SlashEnter,
    /// Ctrl+L / `:clear` confirmed: the same new session as `/clear`.
    ClearSession,
}

pub(super) fn idle_loop_key_action(
    key: &crossterm::event::KeyEvent,
    mode: AppMode,
    agent_busy: bool,
    has_parked: bool,
    at_session_limit: bool,
) -> Option<IdleLoopKey> {
    if key.kind != KeyEventKind::Press || mode != AppMode::Normal {
        return None;
    }
    let ctrl = key
        .modifiers
        .contains(crossterm::event::KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('t') if ctrl && !agent_busy => Some(IdleLoopKey::CycleAgent),
        KeyCode::Char('n') if ctrl => Some(if at_session_limit {
            IdleLoopKey::SessionLimit
        } else {
            IdleLoopKey::NewSession
        }),
        KeyCode::PageDown if ctrl && has_parked => Some(IdleLoopKey::CycleSession { next: true }),
        KeyCode::PageUp if ctrl && has_parked => Some(IdleLoopKey::CycleSession { next: false }),
        KeyCode::Char('o') if ctrl => Some(IdleLoopKey::Dashboard),
        KeyCode::Tab if ctrl && has_parked => Some(IdleLoopKey::MruSwitch),
        KeyCode::Enter if !agent_busy => Some(IdleLoopKey::SlashEnter),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BusyKey {
    Esc,
    Quit,
    CtrlC,
    WaitEnter(SteerMode),
    PassThrough,
}

/// Enter while a turn runs: Shift queues the message for after the turn,
/// Ctrl interrupts between tool calls. Both need a terminal that reports
/// modifiers on Enter (kitty keyboard protocol); `/queue` and `/now` work
/// everywhere.
pub(super) fn enter_steer_mode(modifiers: crossterm::event::KeyModifiers) -> SteerMode {
    use crossterm::event::KeyModifiers;
    if modifiers.contains(KeyModifiers::SHIFT) {
        SteerMode::Queue
    } else if modifiers.contains(KeyModifiers::CONTROL) {
        SteerMode::Urgent
    } else {
        SteerMode::NextStep
    }
}

pub(super) fn busy_key_action(key: &crossterm::event::KeyEvent) -> BusyKey {
    match key.code {
        KeyCode::Esc => BusyKey::Esc,
        KeyCode::Char('q')
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            BusyKey::Quit
        }
        KeyCode::Char('c')
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            BusyKey::CtrlC
        }
        KeyCode::Enter => BusyKey::WaitEnter(enter_steer_mode(key.modifiers)),
        _ => BusyKey::PassThrough,
    }
}

/// Idle Ctrl+T/N/Page/O/Tab and slash-Enter. Returns true when the event is consumed.
#[allow(clippy::too_many_arguments)]
pub(super) async fn apply_idle_loop_key(
    action: IdleLoopKey,
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    runtimes: &mut Vec<SessionRuntime>,
    mru: &mut Vec<usize>,
    config: &mut Config,
    project_dir: &std::path::Path,
    file_index: &Arc<whycodes_index::WorkspaceIndex>,
    session_claims: &whycodes_core::FileClaimRegistry,
    provider: &mut String,
    model: &mut String,
    api_key: &mut String,
    auth_tx: &mpsc::UnboundedSender<AuthFlowEvent>,
) -> bool {
    match action {
        IdleLoopKey::CycleAgent => {
            cycle_agent(
                app,
                &mut rt.agent,
                &mut rt.session,
                config,
                project_dir,
                Arc::clone(&rt.perm_prompter),
                Arc::clone(&rt.question_prompter),
                &rt.event_tx,
            )
            .await;
            true
        }
        IdleLoopKey::NewSession => {
            let fresh = spawn_new_session_runtime(
                &app.agent_name,
                config,
                project_dir,
                file_index,
                session_claims.clone(),
                provider,
                model,
                Some(rt.agent.shared_tools()),
            )
            .await;
            adopt_fresh_runtime(app, rt, runtimes, mru, fresh);
            true
        }
        IdleLoopKey::SessionLimit => {
            warn_session_limit(app);
            true
        }
        IdleLoopKey::CycleSession { next } => {
            cycle_live_session(app, rt, runtimes, mru, next);
            true
        }
        IdleLoopKey::Dashboard => {
            open_sessions_dashboard(app, rt, runtimes);
            true
        }
        IdleLoopKey::MruSwitch => {
            switch_mru_session(app, rt, runtimes, mru);
            true
        }
        IdleLoopKey::SlashEnter | IdleLoopKey::ClearSession => {
            let text = if action == IdleLoopKey::ClearSession {
                // The prompt draft is not the command: keep it.
                Some("/clear".to_string())
            } else if slash::take_mode_command(app).is_some() {
                // Idle, `/queue` and `/now` are a plain prompt.
                return false;
            } else {
                let text = slash_command_from_prompt(app);
                if text.is_some() {
                    consume_slash_draft(app);
                }
                text
            };
            if let Some(text) = text {
                handle_slash(
                    &text,
                    &mut SlashContext {
                        app,
                        session: &mut rt.session,
                        history: &mut rt.history,
                        agent: &mut rt.agent,
                        config,
                        project_dir,
                        provider,
                        model,
                        api_key,
                        perm_prompter: Arc::clone(&rt.perm_prompter),
                        question_prompter: Arc::clone(&rt.question_prompter),
                        auth_tx: auth_tx.clone(),
                        pending_compact: &mut rt.pending_compact,
                        goal: &mut rt.goal,
                        goal_db: rt.db.as_ref(),
                    },
                )
                .await;
                true
            } else {
                false
            }
        }
    }
}

/// Busy Esc / Ctrl+Q / Ctrl+C / Enter / passthrough. Always consumes the key.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_busy_key(
    action: BusyKey,
    ev: &Event,
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    cancel_requested_at: &mut Option<Instant>,
    config: &Config,
    project_dir: &std::path::Path,
    file_index: &Arc<whycodes_index::WorkspaceIndex>,
) {
    match action {
        BusyKey::Esc => {
            // First Esc: cooperative cancel. Second: force-stop now.
            if cancel_requested_at.is_some() {
                force_stop_turn(
                    app,
                    rt,
                    cancel_requested_at,
                    config,
                    project_dir,
                    file_index,
                );
            } else {
                begin_cancel(
                    app,
                    &rt.cancel_flag,
                    cancel_requested_at,
                    &mut rt.pending_question_queue,
                    &mut rt.pending_perm_queue,
                );
            }
            app.esc_armed_at = None;
        }
        BusyKey::Quit => {
            // Quit: always force-stop so we never hang on exit.
            if rt.agent_busy {
                force_stop_turn(
                    app,
                    rt,
                    cancel_requested_at,
                    config,
                    project_dir,
                    file_index,
                );
            }
            app.running = false;
        }
        BusyKey::CtrlC => match busy_ctrl_c(app, *cancel_requested_at) {
            BusyCtrlC::ClearedDraft => {}
            BusyCtrlC::BeginCancel => begin_cancel(
                app,
                &rt.cancel_flag,
                cancel_requested_at,
                &mut rt.pending_question_queue,
                &mut rt.pending_perm_queue,
            ),
            BusyCtrlC::ForceStop => force_stop_turn(
                app,
                rt,
                cancel_requested_at,
                config,
                project_dir,
                file_index,
            ),
        },
        BusyKey::WaitEnter(mode) => {
            if slash::busy_tell(app, &rt.steer) || slash::busy_agents(app) {
                return;
            }
            let command = slash::take_mode_command(app);
            if command.is_none() && slash::busy_slash(app, &mut rt.goal, rt.db.as_ref()) {
                return;
            }
            let mode = command.unwrap_or(mode);
            if app.prompt_has_content() {
                app.submit_input_with(mode);
                if mode == SteerMode::Queue {
                    app.status_message = "queued for after this turn".into();
                }
            } else if command.is_none() {
                toast_wait_for_turn(app);
            }
        }
        BusyKey::PassThrough => {
            // Typing, scroll, focus toggle — all allowed mid-turn.
            let _ = input::handle_event(app, ev.clone());
        }
    }
}

pub(super) fn toast_wait_for_turn(app: &mut TuiApp) {
    app.toasts.push(
        crate::toast::ToastKind::Info,
        "Wait for turn or Esc to cancel",
    );
}

/// While a turn runs, the agent hub and the worker view read their own keys
/// (Esc closes them instead of cancelling the turn). Ctrl chords still reach
/// the turn, so Ctrl+C cancels as usual.
pub(super) fn hub_owns_key(app: &TuiApp, ev: &Event) -> bool {
    let Event::Key(key) = ev else {
        return false;
    };
    let open =
        app.open_subagent.is_some() || matches!(app.dialogs.active(), Some(DialogKind::Agents));
    open && !key
        .modifiers
        .contains(crossterm::event::KeyModifiers::CONTROL)
}

/// Any other dialog open mid-turn (model / effort / mode pickers, help, the
/// sessions dashboard) reads Enter and Esc itself: Enter selects instead of
/// submitting the draft, Esc closes it instead of cancelling the turn. Ctrl
/// chords still reach the turn.
pub(super) fn picker_owns_key(app: &TuiApp, ev: &Event) -> bool {
    let Event::Key(key) = ev else {
        return false;
    };
    app.dialogs.active().is_some()
        && !key
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL)
}

pub(super) fn dialog_overlay_owns_keys(active: Option<&DialogKind>) -> bool {
    matches!(
        active,
        Some(DialogKind::Permission { .. } | DialogKind::Question(_))
    )
}

pub(super) fn busy_ctrl_c(app: &mut TuiApp, cancel_requested_at: Option<Instant>) -> BusyCtrlC {
    if !app.input_buffer.is_empty() {
        app.clear_prompt_draft();
        app.toasts.push(
            crate::toast::ToastKind::Info,
            "Draft cleared — Ctrl+C again to cancel",
        );
        return BusyCtrlC::ClearedDraft;
    }
    if cancel_requested_at.is_some() {
        BusyCtrlC::ForceStop
    } else {
        BusyCtrlC::BeginCancel
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BusyCtrlC {
    ClearedDraft,
    BeginCancel,
    ForceStop,
}
