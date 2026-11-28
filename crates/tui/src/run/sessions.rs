//! Live sessions in one TUI: opening, switching, closing and the dashboard rows.
use super::*;

/// Build a fresh runtime for a new empty session (Ctrl+N). Owns its
/// prompter pair and channels; the agent shares no state with any other
/// runtime except the process-wide background registry pattern.
#[allow(clippy::too_many_arguments)]
pub(super) async fn spawn_new_session_runtime(
    agent_name: &str,
    config: &Config,
    project_dir: &std::path::Path,
    file_index: &Arc<whycodes_index::WorkspaceIndex>,
    session_claims: whycodes_core::FileClaimRegistry,
    provider: &str,
    model: &str,
    shared_tools: Option<whycodes_agent::agent::SharedTools>,
) -> SessionRuntime {
    let agent_info =
        config
            .get_agent(agent_name)
            .cloned()
            .unwrap_or_else(|| whycodes_core::types::AgentInfo {
                name: agent_name.to_string(),
                description: "Default".into(),
                mode: AgentMode::Primary,
                permission: whycodes_core::types::PermissionSet {
                    allow_file_writes: true,
                    allow_network: true,
                    allow_shell: true,
                    ..whycodes_core::types::PermissionSet::default()
                },
                model: None,
                system_prompt: None,
                temperature: None,
                top_p: None,
            });
    let (perm_prompter, perm_rx) = ChannelPermissionPrompter::new();
    let perm_prompter =
        perm_prompter.with_notify(whycodes_agent::notify::handle_from_config(&config.notify));
    let perm_prompter: Arc<ChannelPermissionPrompter> = Arc::new(perm_prompter);
    let q_timeout = if config.tools.question.timeout_enabled {
        Some(Duration::from_secs(
            config.tools.question.timeout_secs.max(1),
        ))
    } else {
        None
    };
    let (question_prompter, question_rx) = ChannelQuestionPrompter::new(q_timeout);
    let question_prompter =
        question_prompter.with_notify(whycodes_agent::notify::handle_from_config(&config.notify));
    let question_prompter: Arc<ChannelQuestionPrompter> = Arc::new(question_prompter);

    let mut agent = Agent::new(agent_info)
        .with_config(config)
        .with_file_index(file_index.clone())
        .with_session_claims(session_claims)
        .with_permission_prompter(
            Arc::clone(&perm_prompter) as Arc<dyn whycodes_agent::PermissionPrompter>
        )
        .with_question_prompter(Arc::clone(&question_prompter) as Arc<dyn QuestionPrompter>);
    agent.set_route(provider, model);
    agent.set_approval_mode(config.general.approval_mode.unwrap_or_default());
    // Sessions in one TUI share tools: MCP servers and LSP clients start once.
    agent = match shared_tools {
        Some(tools) => agent.with_shared_tools(tools),
        None => agent.with_mcp(config).await,
    };
    let system_prompt = with_project_memory(
        &Agent::with_agents_md(&agent.system_prompt_for_route(provider, model), project_dir),
        project_dir,
        config,
        None,
    );

    let session = Session::new(project_dir.to_path_buf(), system_prompt);
    let history = SessionHistory::with_depth(config.session.undo_depth);
    let (event_tx, event_rx) = mpsc::unbounded_channel::<TurnEvent>();
    let (done_tx, done_rx) = mpsc::unbounded_channel::<TurnOutcome>();

    SessionRuntime::new(
        agent,
        session,
        history,
        event_tx,
        event_rx,
        done_tx,
        done_rx,
        perm_prompter,
        question_prompter,
        perm_rx,
        question_rx,
    )
}

/// Refresh the dashboard row snapshot from the live runtimes and open the
/// dashboard dialog. Rows are grouped: needs-input → working → idle.
pub(super) fn open_sessions_dashboard(
    app: &mut TuiApp,
    rt: &SessionRuntime,
    runtimes: &[SessionRuntime],
) {
    refresh_sessions_rows(app, rt, runtimes);
    if !matches!(app.dialogs.active(), Some(DialogKind::Sessions)) {
        app.sessions_cursor = 0;
        app.dialogs.push(DialogKind::Sessions);
        app.mode = AppMode::Dialog;
        app.key_context = KeymapContext::Dialog;
    }
    app.mark_dirty();
}

/// Rebuild the grouped row snapshot in place (live refresh while open).
///
/// Returns `true` when the painted rows changed (caller should `mark_dirty`).
pub(super) fn refresh_sessions_rows(
    app: &mut TuiApp,
    rt: &SessionRuntime,
    runtimes: &[SessionRuntime],
) -> bool {
    let mut rows: Vec<crate::app::SessionDashboardRow> = Vec::new();
    let active_state = rt.state();
    rows.push(crate::app::SessionDashboardRow {
        parked_idx: None,
        title: format!("{} (current)", rt.session.title),
        glyph: active_state.glyph().to_string(),
        state_label: active_state.label().to_string(),
        preview: if rt.agent_busy {
            app.status_message.clone()
        } else {
            crate::session_runtime::preview_from_messages(&app.messages)
        },
        unread: false,
    });
    for (i, bg) in runtimes.iter().enumerate() {
        let st = bg.state();
        rows.push(crate::app::SessionDashboardRow {
            parked_idx: Some(i),
            title: bg.session.title.clone(),
            glyph: st.glyph().to_string(),
            state_label: st.label().to_string(),
            preview: bg.preview(),
            unread: bg.unread,
        });
    }
    // Group: needs input (rank 0) → working (1) → idle/error (2); stable
    // within a group so creation order is preserved.
    rows.sort_by_key(|r| {
        let rank = match r.parked_idx {
            None => active_state.group_rank(),
            Some(i) => runtimes[i].state().group_rank(),
        };
        (rank, r.parked_idx.unwrap_or(usize::MAX))
    });
    if app.sessions_rows == rows {
        return false;
    }
    app.sessions_rows = rows;
    true
}

/// Rewrite the picker's live section in place: live rows (active + parked
/// runtimes) sit at the top, persisted-only rows below. Persisted rows keep
/// their relative order; the cursor stays on the same entry when possible.
///
/// Returns `true` when the painted list or cursor changed.
/// Does **not** reopen SQLite — `/sessions` and `/resume` load the DB once
/// when the dialog opens; a per-tick `list_sessions` was a hidden stall.
pub(super) fn refresh_picker_live_section(
    app: &mut TuiApp,
    rt: &SessionRuntime,
    runtimes: &[SessionRuntime],
) -> bool {
    let selected_id = app
        .session_list
        .sessions
        .get(app.session_list.selected)
        .map(|e| e.id.clone());

    let mut live_rows: Vec<crate::app::SessionEntry> = Vec::new();
    live_rows.push(crate::app::SessionEntry {
        id: rt.session.id.clone(),
        title: format!("{} (current)", rt.session.title),
        messages: rt.session.messages.len(),
        updated_at: Some(rt.session.updated_at),
        live: Some(usize::MAX),
    });
    for (i, bg) in runtimes.iter().enumerate() {
        let st = bg.state();
        live_rows.push(crate::app::SessionEntry {
            id: bg.session.id.clone(),
            title: format!("{} {} {}", st.glyph(), bg.session.title, st.label()),
            messages: bg.session.messages.len(),
            updated_at: Some(bg.session.updated_at),
            live: Some(i),
        });
    }
    let live_ids: std::collections::HashSet<&str> =
        live_rows.iter().map(|e| e.id.as_str()).collect();
    let persisted: Vec<crate::app::SessionEntry> = app
        .session_list
        .sessions
        .iter()
        .filter(|e| e.live.is_none() && !live_ids.contains(e.id.as_str()))
        .cloned()
        .collect();
    let mut merged = live_rows;
    merged.extend(persisted);
    if app.session_list.sessions == merged {
        return false;
    }
    app.session_list.sessions = merged;
    if let Some(id) = selected_id
        && let Some(pos) = app.session_list.sessions.iter().position(|e| e.id == id)
    {
        app.session_list.selected = pos;
    }
    true
}

/// Swap the active runtime with `runtimes[idx]`, preserving both sessions'
/// view state. Transcripts **move** (no clone): the visible app yields into
/// the outgoing snapshot and adopts the incoming one.
pub(super) fn switch_to_runtime(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    runtimes: &mut [SessionRuntime],
    idx: usize,
) {
    app.yield_view(&mut rt.view);
    std::mem::swap(rt, &mut runtimes[idx]);
    rt.unread = false;
    app.adopt_view(&mut rt.view);
    app.dialogs.clear();
    app.mark_dirty();
    app.focus = FocusPane::Prompt;
    app.request_full_clear(2);
}

/// Drain a background (inactive) runtime: prompter requests into its queues,
/// turn events into its view snapshot, completion into agent/session restore.
/// Never touches the visible `app`; sets `unread` on any activity so the
/// dashboard and cycle keys can surface it.
///
/// Idle path is a few `try_recv`s — no `TuiApp`, no transcript clone, no
/// syntax-theme swap. Events move the snapshot into a detached scratch app
/// and back (`adopt_view` / `yield_view`) so a parked stream does not
/// duplicate the whole transcript every tick.
pub(super) fn drain_background_runtime(rt: &mut SessionRuntime) {
    while let Ok(req) = rt.perm_rx.try_recv() {
        notify_permission(&req);
        rt.pending_perm_queue.push_back(req);
        rt.unread = true;
    }
    while let Ok(req) = rt.question_rx.try_recv() {
        notify_question(&req);
        rt.pending_question_queue.push_back(req);
        rt.unread = true;
    }

    if !rt.event_rx.is_empty() {
        // Disjoint from `rt.view`: adopt → drain → yield, no overlapping borrow.
        let mut scratch = TuiApp::from_config(crate::config::TuiAppConfig::default());
        scratch.adopt_view(&mut rt.view);
        let any = drain_turn_events(&mut scratch, &mut rt.event_rx) != Drained::Nothing;
        scratch.yield_view(&mut rt.view);
        if any {
            rt.unread = true;
        }
    }

    if let Ok(outcome) = rt.done_rx.try_recv() {
        rt.agent_busy = false;
        rt.cancel_flag = None;
        rt.last_turn_end = Some(Instant::now());
        rt.turn_join = None;
        rt.session_backup = None;
        rt.unread = true;
        crate::heap::request_release_after_draw("background_turn_done");
        match outcome {
            TurnOutcome::Ok {
                text,
                agent: a,
                session: s,
                ..
            } => {
                rt.agent = a;
                rt.session = s;
                rt.last_error = false;
                crate::desktop_notify::notify(
                    whycodes_config::NotifyEvent::TurnDone,
                    &crate::desktop_notify::turn_done_body(&text),
                );
                let goal_note = pause_parked_goal(rt, "session was in the background");
                with_view_scratch(&mut rt.view, |scratch| {
                    if !text.is_empty()
                        && let Some(last) = scratch.messages.last_mut()
                        && last.role == ChatRole::Assistant
                        && last.content.is_empty()
                    {
                        last.content = text;
                    }
                    if let Some(note) = goal_note {
                        scratch.add_message(ChatRole::System, note);
                    }
                });
            }
            TurnOutcome::Remote { text, error, .. } => {
                rt.last_error = error.is_some();
                if let Some(err) = error {
                    with_view_scratch(&mut rt.view, |scratch| {
                        scratch.add_message(ChatRole::System, format!("Remote error: {err}"));
                    });
                } else if !text.is_empty() {
                    with_view_scratch(&mut rt.view, |scratch| {
                        if let Some(last) = scratch.messages.last_mut()
                            && last.role == ChatRole::Assistant
                            && last.content.is_empty()
                        {
                            last.content = text;
                        }
                    });
                }
            }
            TurnOutcome::Err {
                agent: a,
                session: s,
                cancelled,
                error,
                ..
            } => {
                rt.agent = a;
                rt.session = s;
                rt.last_error = !cancelled;
                let why = if cancelled {
                    "turn cancelled"
                } else {
                    "turn failed"
                };
                let goal_note = pause_parked_goal(rt, why);
                with_view_scratch(&mut rt.view, |scratch| {
                    // Same as the visible session: no more /loop prompts
                    // after a failed or cancelled turn.
                    stop_loop_after_error(scratch, why);
                    if let Some(note) = goal_note {
                        scratch.add_message(ChatRole::System, note);
                    }
                    if cancelled {
                        scratch.add_message(ChatRole::System, "⏹ Generation cancelled.");
                    } else {
                        let display = whycodes_llm::format_turn_error(&whycodes_core::Error::llm(
                            error.clone(),
                        ));
                        scratch.add_message(ChatRole::System, format!("Error: {display}"));
                        crate::desktop_notify::notify(
                            whycodes_config::NotifyEvent::TurnDone,
                            &format!("Turn failed: {display}"),
                        );
                    }
                });
            }
            TurnOutcome::Compact {
                agent: a,
                session: s,
                outcome,
                ..
            } => {
                rt.agent = a;
                rt.last_error = false;
                with_view_scratch(&mut rt.view, |scratch| {
                    apply_compact_view(scratch, &s, &outcome);
                });
                rt.session = s;
            }
        }
        rt.persist("background");
    }
}

/// Close the active session (`usize::MAX`) or a parked slot.
pub(super) fn close_session_slot(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    runtimes: &mut Vec<SessionRuntime>,
    mru: &mut Vec<usize>,
    close_idx: usize,
) {
    if close_idx == usize::MAX {
        if rt.agent_busy {
            app.toasts.push(
                crate::toast::ToastKind::Warning,
                "Turn in flight — Esc first, then close",
            );
        } else if runtimes.is_empty() {
            app.toasts.push(
                crate::toast::ToastKind::Info,
                "Last live session stays open",
            );
        } else {
            rt.persist("close");
            let idx = mru.pop().unwrap_or(runtimes.len() - 1);
            let idx = idx.min(runtimes.len() - 1);
            let mut closed = std::mem::replace(rt, runtimes.remove(idx));
            closed.turn_join.take();
            drop(closed);
            crate::heap::request_release_after_draw("session_closed");
            mru.retain(|&i| i != idx);
            for i in mru.iter_mut() {
                if *i > idx {
                    *i -= 1;
                }
            }
            rt.unread = false;
            app.adopt_view(&mut rt.view);
            app.dialogs.clear();
            app.mark_dirty();
            app.focus = FocusPane::Prompt;
            app.toasts.push(
                crate::toast::ToastKind::Info,
                format!(
                    "Closed · now {} ({} live)",
                    rt.session.title,
                    runtimes.len() + 1
                ),
            );
        }
    } else if close_idx < runtimes.len() {
        let mut bg = runtimes.remove(close_idx);
        reject_pending(&mut bg.pending_question_queue, &mut bg.pending_perm_queue);
        if let Some(h) = bg.turn_join.take() {
            h.abort();
        }
        bg.agent.background_registry().kill_all();
        bg.persist("close");
        mru.retain(|&i| i != close_idx);
        for i in mru.iter_mut() {
            if *i > close_idx {
                *i -= 1;
            }
        }
        app.toasts.push(
            crate::toast::ToastKind::Info,
            format!(
                "Closed · {} ({} live)",
                bg.session.title,
                runtimes.len() + 1
            ),
        );
        drop(bg);
        crate::heap::request_release_after_draw("session_closed");
    }
}

/// Switch to a parked live session, or load a persisted id into the active one.
pub(super) fn resume_or_switch_session(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    runtimes: &mut [SessionRuntime],
    mru: &mut Vec<usize>,
    id: String,
    project_dir: &std::path::Path,
    config: &Config,
) {
    if rt.agent_busy {
        // Switching mid-turn would leave it behind; the switch waits, and
        // the user is told so instead of it happening out of nowhere later.
        app.pending_session_id = Some(id);
        app.toasts.push(
            crate::toast::ToastKind::Info,
            "Switches to that session when this turn ends",
        );
        return;
    }
    if let Some(idx) = runtimes.iter().position(|b| b.session.id == id) {
        switch_to_runtime(app, rt, runtimes, idx);
        mru.retain(|&i| i != idx);
        mru.push(idx);
        app.toasts.push(
            crate::toast::ToastKind::Success,
            format!("Switched to live session · {}", rt.session.title),
        );
        return;
    }
    match try_load_session(&id) {
        Ok(Some(loaded)) => {
            if !rt.session.messages.is_empty() {
                rt.persist("switch");
            }
            let n = loaded.messages.len();
            rt.history.clear();
            rt.session = loaded;
            let (p, m) = rt.agent.route();
            rt.session.system_prompt = with_project_memory(
                &Agent::with_agents_md(&rt.agent.system_prompt_for_route(p, m), project_dir),
                project_dir,
                config,
                None,
            );
            if config.session.auto_title && rt.session.maybe_upgrade_title_from_history() {
                rt.persist("title_backfill");
            }
            let title = rt.session.title.clone();
            app.load_messages_from_session(&rt.session);
            crate::heap::request_release_after_draw("history_loaded");
            app.toasts.push(
                crate::toast::ToastKind::Success,
                format!("Resumed · {title} ({n} msgs)"),
            );
            app.status_message = format!("Resumed rt.session {}", short_session_id(&rt.session.id));
        }
        Ok(None) => {
            app.toasts.push(
                crate::toast::ToastKind::Warning,
                format!("Session not found: {}", short_session_id(&id)),
            );
        }
        Err(e) => {
            app.toasts.push(
                crate::toast::ToastKind::Error,
                format!("Resume failed: {e}"),
            );
        }
    }
}

pub(super) fn shutdown_runtime_queues(rt: &mut SessionRuntime) {
    reject_pending(&mut rt.pending_question_queue, &mut rt.pending_perm_queue);
    if let Some(flag) = rt.cancel_flag.as_ref() {
        request_cancel(flag);
    }
    if let Some(h) = rt.turn_join.take() {
        h.abort();
    }
    rt.agent.background_registry().kill_all();
}

pub(super) fn cycle_live_session(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    runtimes: &mut [SessionRuntime],
    mru: &mut Vec<usize>,
    page_down: bool,
) {
    if runtimes.is_empty() {
        return;
    }
    let idx = if page_down { 0 } else { runtimes.len() - 1 };
    switch_to_runtime(app, rt, runtimes, idx);
    mru.retain(|&i| i != idx);
    mru.push(idx);
    app.toasts.push(
        crate::toast::ToastKind::Info,
        format!(
            "Session · {} ({} live)",
            rt.session.title,
            runtimes.len() + 1
        ),
    );
}

pub(super) fn switch_mru_session(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    runtimes: &mut [SessionRuntime],
    mru: &mut Vec<usize>,
) {
    if runtimes.is_empty() {
        return;
    }
    let idx = mru.pop().unwrap_or(runtimes.len() - 1);
    let idx = idx.min(runtimes.len() - 1);
    switch_to_runtime(app, rt, runtimes, idx);
    mru.retain(|&i| i != idx);
    mru.push(idx);
    app.toasts.push(
        crate::toast::ToastKind::Info,
        format!(
            "Session · {} ({} live)",
            rt.session.title,
            runtimes.len() + 1
        ),
    );
}

pub(super) fn adopt_fresh_runtime(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    runtimes: &mut Vec<SessionRuntime>,
    mru: &mut Vec<usize>,
    fresh: SessionRuntime,
) {
    app.yield_view(&mut rt.view);
    let parked = std::mem::replace(rt, fresh);
    runtimes.push(parked);
    mru.push(runtimes.len() - 1);
    app.restore_view(&rt.view);
    app.session_title = rt.session.title.clone();
    app.focus = FocusPane::Prompt;
    // Home gutters are spaces in both ratatui frames; skip-diff will not
    // erase a paste echo (or session sidebar chrome) left on the PTY.
    app.request_full_clear(2);
    app.toasts.push(
        crate::toast::ToastKind::Info,
        format!("New session ({} live)", runtimes.len() + 1),
    );
}

pub(super) fn warn_session_limit(app: &mut TuiApp) {
    app.toasts.push(
        crate::toast::ToastKind::Warning,
        format!("Session limit ({MAX_LIVE_SESSIONS}) — close one first"),
    );
}

pub(super) fn refresh_live_session_ui(
    app: &mut TuiApp,
    rt: &SessionRuntime,
    runtimes: &[SessionRuntime],
) {
    let chip = goal_chip(rt.goal.as_ref());
    if app.goal_chip != chip {
        app.goal_chip = chip;
        app.mark_dirty();
    }
    if matches!(app.dialogs.active(), Some(DialogKind::Sessions)) {
        let cursor = app.sessions_cursor;
        let changed = refresh_sessions_rows(app, rt, runtimes);
        let clamped = cursor.min(app.sessions_rows.len().saturating_sub(1));
        if changed || clamped != app.sessions_cursor {
            app.sessions_cursor = clamped;
            app.mark_dirty();
        }
    }
    if matches!(app.dialogs.active(), Some(DialogKind::SessionList))
        && refresh_picker_live_section(app, rt, runtimes)
    {
        app.mark_dirty();
    }
}

pub(super) fn apply_dashboard_switch(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    runtimes: &mut [SessionRuntime],
    mru: &mut Vec<usize>,
    target: usize,
) {
    if target == usize::MAX || target >= runtimes.len() {
        return;
    }
    switch_to_runtime(app, rt, runtimes, target);
    mru.retain(|&i| i != target);
    mru.push(target);
    app.toasts.push(
        crate::toast::ToastKind::Success,
        format!(
            "Session · {} ({} live)",
            rt.session.title,
            runtimes.len() + 1
        ),
    );
}
