//! One turn from the event loop's side: starting it, applying its events
//! and outcome, and cancelling it.
use super::*;

/// Arm cooperative cancel: set the flag, unblock permission/question waits,
/// and start the force-stop timer.
/// Mouse `[stop]` / UI cancel: first click begins cooperative cancel, a
/// second click while already cancelling force-stops the turn.
pub(super) fn apply_pending_cancel(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    cancel_requested_at: &mut Option<Instant>,
    config: &Config,
    project_dir: &std::path::Path,
    file_index: &Arc<whycodes_index::WorkspaceIndex>,
) {
    if !rt.agent_busy || !app.pending_cancel {
        return;
    }
    app.pending_cancel = false;
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
}

pub(super) fn begin_cancel(
    app: &mut TuiApp,
    cancel_flag: &Option<CancelFlag>,
    cancel_requested_at: &mut Option<Instant>,
    pending_question_queue: &mut std::collections::VecDeque<QuestionRequest>,
    pending_perm_queue: &mut std::collections::VecDeque<whycodes_agent::PermissionRequest>,
) {
    if let Some(flag) = cancel_flag.as_ref() {
        request_cancel(flag);
    }
    reject_pending(pending_question_queue, pending_perm_queue);
    if cancel_requested_at.is_none() {
        *cancel_requested_at = Some(Instant::now());
    }
    app.status_message = "Cancelling…".into();
    app.current_agent_state = AgentState::Generating;
    app.finish_open_thinking();
    close_interactive_overlays(app);
    app.mark_dirty();
}

/// Hard-stop: abort the turn task, restore agent/session, free the UI.
///
/// Called after [`CANCEL_FORCE_AFTER`] or on a second Esc/[stop] while already
/// cancelling. Guarantees `rt.agent_busy` becomes false.
pub(super) fn force_stop_turn(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    cancel_requested_at: &mut Option<Instant>,
    config: &Config,
    project_dir: &std::path::Path,
    file_index: &Arc<whycodes_index::WorkspaceIndex>,
) {
    // Always re-signal cancel in case the task is still cooperative.
    if let Some(flag) = rt.cancel_flag.as_ref() {
        request_cancel(flag);
    }
    reject_pending(&mut rt.pending_question_queue, &mut rt.pending_perm_queue);

    if let Some(h) = rt.turn_join.take() {
        h.abort();
    }

    // If the task finished in the race window, prefer its restored agent/session.
    let mut got_outcome = false;
    while let Ok(outcome) = rt.done_rx.try_recv() {
        got_outcome = true;
        match outcome {
            TurnOutcome::Ok {
                agent: a,
                session: s,
                ..
            }
            | TurnOutcome::Err {
                agent: a,
                session: s,
                ..
            }
            | TurnOutcome::Compact {
                agent: a,
                session: s,
                ..
            } => {
                rt.agent = a;
                rt.session = s;
            }
            TurnOutcome::Remote { .. } => {}
        }
    }

    if !got_outcome {
        // Task dropped without returning — rebuild agent; restore session snapshot.
        if let Some(backup) = rt.session_backup.take() {
            rt.session = backup;
        }
        let preferred = if app.agent_name.is_empty() {
            rt.agent.info.name.clone()
        } else {
            app.agent_name.clone()
        };
        rebuild_agent_after_force_stop(
            &mut rt.agent,
            &mut rt.session,
            config,
            project_dir,
            &preferred,
            rt.event_tx.clone(),
            Arc::clone(&rt.perm_prompter),
            Arc::clone(&rt.question_prompter),
            file_index,
        );
    } else {
        rt.session_backup.take();
    }

    rt.agent_busy = false;
    rt.cancel_flag = None;
    *cancel_requested_at = None;

    app.finish_open_thinking();
    app.current_agent_state = AgentState::Idle;
    app.status_message = format_turn_done_status(
        app,
        rt.agent.info.name.as_str(),
        &app.provider_name,
        &app.model_name,
        app.turn_elapsed_ms(),
        true,
    );
    // Avoid duplicate system lines if cooperative cancel already announced.
    let already = app
        .messages
        .last()
        .map(|m| m.role == ChatRole::System && m.content.contains("cancelled"))
        .unwrap_or(false);
    if !already {
        app.add_message(ChatRole::System, "⏹ Stopped.");
    }
    // A forced stop is the user's "stop": the next /loop prompt must not
    // start right after it, and a /goal must not judge a turn it never saw.
    stop_loop_after_error(app, "turn stopped");
    pause_goal_after_error(app, rt, true);
    // The aborted turn took its workers with it; a row left `running` kept
    // the fast poll rate on for good, and a panel that never folded.
    app.cancel_running_subagents();
    persist_session_best_effort(&rt.session, "force_cancelled");
    close_interactive_overlays(app);
    app.mark_dirty();
}

#[allow(clippy::too_many_arguments)]
pub(super) fn rebuild_agent_after_force_stop(
    agent: &mut Agent,
    session: &mut Session,
    config: &Config,
    project_dir: &std::path::Path,
    preferred_name: &str,
    event_tx: mpsc::UnboundedSender<TurnEvent>,
    perm_prompter: Arc<ChannelPermissionPrompter>,
    question_prompter: Arc<ChannelQuestionPrompter>,
    file_index: &Arc<whycodes_index::WorkspaceIndex>,
) {
    let name = if preferred_name.is_empty() || preferred_name == "_pending" {
        if config.default_agent.is_empty() {
            "build".into()
        } else {
            config.default_agent.clone()
        }
    } else {
        preferred_name.to_string()
    };
    let info =
        config
            .get_agent(&name)
            .cloned()
            .unwrap_or_else(|| whycodes_core::types::AgentInfo {
                name: name.clone(),
                description: String::new(),
                mode: AgentMode::Primary,
                permission: whycodes_core::types::PermissionSet::default(),
                model: None,
                system_prompt: None,
                temperature: None,
                top_p: None,
            });
    let bg = agent.background_registry().clone();
    let claims = agent.session_claims();
    let (route_provider, route_model) = agent.route();
    let route_provider = route_provider.to_string();
    let route_model = route_model.to_string();
    let mut next = Agent::new(info)
        .with_config(config)
        .with_background_registry(bg)
        .with_file_index(file_index.clone())
        .with_permission_prompter(perm_prompter as Arc<dyn whycodes_agent::PermissionPrompter>)
        .with_question_prompter(question_prompter as Arc<dyn QuestionPrompter>);
    next.set_route(&route_provider, &route_model);
    if let Some(c) = claims {
        next = next.with_session_claims(c);
    }
    *agent = next;
    agent.wire_event_sink(event_tx);
    // Keep existing system prompt on session if any; else set rebuilt one.
    if session.system_prompt.is_empty() {
        let prompt = with_project_memory(
            &Agent::with_agents_md(
                &agent.system_prompt_for_route(&route_provider, &route_model),
                project_dir,
            ),
            project_dir,
            config,
            None,
        );
        session.set_system_prompt(&prompt);
    }
}

/// Status line after a turn ends (header chrome).
///
/// Format: `Worked for 4.2s · 1.2k in / 340 out` — no `agent=` noise.
/// Cancelled: `Turn cancelled in 4.2s`.
pub(super) fn format_turn_done_status(
    app: &TuiApp,
    _agent_name: &str,
    _provider: &str,
    _model: &str,
    elapsed_ms: Option<u128>,
    cancelled: bool,
) -> String {
    if cancelled {
        return match elapsed_ms {
            Some(ms) => format!("Turn cancelled in {}", format_elapsed_ms(ms)),
            None => "Turn cancelled.".into(),
        };
    }
    let mut parts = Vec::new();
    if let Some(ms) = elapsed_ms {
        parts.push(format!("Worked for {}", format_elapsed_ms(ms)));
    } else {
        parts.push("Done".into());
    }
    if let Some(ref usage) = app.turn_usage {
        parts.push(format_usage_short(usage));
    }
    if let Some(tps) = app.stream_speed.tokens_per_second() {
        parts.push(format!("{tps:.0} tok/s"));
    }
    parts.join(" · ")
}

/// Drain the agent event channel, coalescing consecutive text/thinking deltas
/// into one UI append each. Returns the strongest kind of event applied.
pub(super) fn drain_turn_events(
    app: &mut TuiApp,
    event_rx: &mut mpsc::UnboundedReceiver<TurnEvent>,
) -> Drained {
    let mut drained = Drained::Nothing;
    let mut text_buf = String::new();
    let mut think_buf = String::new();

    let flush_text = |app: &mut TuiApp, buf: &mut String| {
        if buf.is_empty() {
            return;
        }
        app.finish_open_thinking();
        app.current_agent_state = AgentState::Generating;
        app.append_to_last(buf);
        buf.clear();
    };
    let flush_think = |app: &mut TuiApp, buf: &mut String| {
        if buf.is_empty() {
            return;
        }
        app.current_agent_state = AgentState::Thinking;
        app.append_thinking(buf);
        buf.clear();
    };

    while let Ok(ev) = event_rx.try_recv() {
        drained = drained.max(Drained::Deltas);
        match ev {
            TurnEvent::TextDelta(t) => {
                app.stream_speed.note_delta(Instant::now());
                flush_think(app, &mut think_buf);
                text_buf.push_str(&t);
            }
            TurnEvent::ThinkingDelta(t) => {
                app.stream_speed.note_delta(Instant::now());
                flush_text(app, &mut text_buf);
                think_buf.push_str(&t);
            }
            other => {
                drained = Drained::Events;
                flush_text(app, &mut text_buf);
                flush_think(app, &mut think_buf);
                apply_turn_event(app, other);
            }
        }
    }
    flush_text(app, &mut text_buf);
    flush_think(app, &mut think_buf);
    drained
}

/// Apply a finished turn to the visible session. Returns `true` when the
/// catalog fetch should be queued (no live context window yet).
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_turn_outcome(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    outcome: TurnOutcome,
    cancel_requested_at: &mut Option<Instant>,
    pending_async_title: &mut Option<(String, String)>,
    provider: &str,
    model: &str,
    config: &Config,
    api_key: &str,
    suggest_tx: &mpsc::UnboundedSender<String>,
) -> bool {
    rt.agent_busy = false;
    rt.cancel_flag = None;
    rt.last_turn_end = Some(Instant::now());
    *cancel_requested_at = None;
    rt.turn_join = None;
    rt.session_backup = None;
    let queue_catalog = app.api_context_window.is_none();
    let work_ms = match &outcome {
        TurnOutcome::Ok { work_ms, .. }
        | TurnOutcome::Err { work_ms, .. }
        | TurnOutcome::Remote { work_ms, .. }
        | TurnOutcome::Compact { work_ms, .. } => *work_ms,
    };
    let elapsed_ms = Some(app.complete_turn_timing_ms(work_ms));
    app.mark_dirty();
    crate::heap::request_release_after_draw("turn_done");
    match outcome {
        TurnOutcome::Ok {
            text,
            agent: a,
            session: s,
            work_ms: _,
        } => {
            rt.agent = a;
            rt.session = s;
            if let Some((sid, title)) = pending_async_title.take()
                && rt.session.id == sid
            {
                rt.session.apply_generated_title(&title);
            }
            app.session_title = rt.session.title.clone();
            if !text.is_empty()
                && let Some(last) = app.messages.last_mut()
                && last.role == ChatRole::Assistant
                && last.content.is_empty()
            {
                last.content = text.clone();
            }
            note_empty_answer(app, &text);
            app.finish_open_thinking();
            app.current_agent_state = AgentState::Idle;
            crate::desktop_notify::notify(
                whycodes_config::NotifyEvent::TurnDone,
                &crate::desktop_notify::turn_done_body(&text),
            );
            // Last-step prompt usage is billed fill for that request; after
            // tools/assistant land, the meter should track the live transcript.
            app.sync_context_estimate(&rt.session);
            app.refresh_git_branch();
            app.status_message = format_turn_done_status(
                app,
                rt.agent.info.name.as_str(),
                provider,
                model,
                elapsed_ms,
                false,
            );
            rt.persist("ok");
            goal_after_turn(app, rt, config, provider, model, api_key);
            maybe_spawn_prompt_suggestion(
                config,
                &rt.session,
                provider,
                model,
                api_key,
                app,
                suggest_tx.clone(),
            );
        }
        TurnOutcome::Remote {
            text,
            error,
            work_ms: _,
        } => {
            app.finish_open_thinking();
            if let Some(err) = error {
                app.current_agent_state = AgentState::Idle;
                app.add_message(ChatRole::System, format!("Remote error: {err}"));
                stop_loop_after_error(app, "turn failed");
                app.status_message = "remote error".into();
            } else {
                if !text.is_empty()
                    && let Some(last) = app.messages.last_mut()
                    && last.role == ChatRole::Assistant
                    && last.content.is_empty()
                {
                    last.content = text.clone();
                }
                note_empty_answer(app, &text);
                app.current_agent_state = AgentState::Idle;
                app.status_message = format_turn_done_status(
                    app,
                    rt.agent.info.name.as_str(),
                    provider,
                    model,
                    elapsed_ms,
                    false,
                );
            }
        }
        TurnOutcome::Err {
            error,
            agent: a,
            session: s,
            cancelled,
            work_ms: _,
        } => {
            rt.agent = a;
            rt.session = s;
            if let Some((sid, title)) = pending_async_title.take()
                && rt.session.id == sid
            {
                rt.session.apply_generated_title(&title);
            }
            app.session_title = rt.session.title.clone();
            app.finish_open_thinking();
            app.sync_context_estimate(&rt.session);
            pause_goal_after_error(app, rt, cancelled);
            if cancelled {
                app.current_agent_state = AgentState::Idle;
                app.status_message = format_turn_done_status(
                    app,
                    rt.agent.info.name.as_str(),
                    provider,
                    model,
                    elapsed_ms,
                    true,
                );
                app.add_message(ChatRole::System, "⏹ Generation cancelled.");
                stop_loop_after_error(app, "turn cancelled");
                rt.persist("cancelled");
            } else {
                let display =
                    whycodes_llm::format_turn_error(&whycodes_core::Error::llm(error.clone()));
                app.current_agent_state = AgentState::Error(display.clone());
                let dur = elapsed_ms
                    .map(|ms| format!("{} · ", format_elapsed_ms(ms)))
                    .unwrap_or_default();
                app.status_message = format!("{dur}error — see chat");
                app.add_message(ChatRole::System, format!("Error: {display}"));
                stop_loop_after_error(app, "turn failed");
                crate::desktop_notify::notify(
                    whycodes_config::NotifyEvent::TurnDone,
                    &format!("Turn failed: {display}"),
                );
                app.toasts
                    .push(crate::toast::ToastKind::Error, truncate_toast(&display, 48));
                whycodes_core::logging::emit_sid(
                    "tui",
                    "error",
                    "turn.error",
                    Some(rt.session.id.as_str()),
                    Some(serde_json::json!({
                        "error": error,
                        "display": display,
                        "elapsed_ms": elapsed_ms,
                    })),
                );
                rt.persist("error");
            }
        }
        TurnOutcome::Compact {
            agent: a,
            session: s,
            outcome,
            work_ms: _,
        } => {
            rt.agent = a;
            rt.session = s;
            apply_compact_view(app, &rt.session, &outcome);
            rt.persist("compact");
        }
    }
    queue_catalog
}

pub(super) fn apply_compact_view(
    app: &mut TuiApp,
    session: &Session,
    outcome: &whycodes_session::CompactOutcome,
) {
    app.load_messages_from_session(session);
    app.current_agent_state = AgentState::Idle;
    app.status_message = format!(
        "Conversation compacted ({} → {} msgs, ~{} → ~{} tok)",
        outcome.messages_before,
        outcome.messages_after,
        outcome.tokens_before,
        outcome.tokens_after
    );
    app.toasts
        .push(crate::toast::ToastKind::Success, "Conversation compacted");
}

pub(super) fn arm_generating(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    cancel_requested_at: &mut Option<Instant>,
    status: &str,
) -> CancelFlag {
    rt.agent_busy = true;
    let flag = new_cancel_flag();
    rt.cancel_flag = Some(Arc::clone(&flag));
    *cancel_requested_at = None;
    app.mark_turn_started();
    app.current_agent_state = AgentState::Generating;
    if status.is_empty() {
        app.status_message.clear();
    } else {
        app.status_message = status.into();
    }
    if app
        .messages
        .last()
        .map(|m| m.role != ChatRole::Assistant)
        .unwrap_or(true)
    {
        app.add_message(ChatRole::Assistant, "");
    }
    flag
}

pub(super) fn record_user_turn(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    prompt: &str,
    project_dir: &std::path::Path,
    config: &Config,
    submit_images: &[crate::images::PromptImage],
) -> (String, whycodes_session::StashSlot) {
    let expanded = expand_at_files(prompt, project_dir);
    // Only the cheap transcript snapshot runs here. The undo stash (two git
    // spawns, ~150 ms on Windows) and the memory rebuild (~200 ms on a
    // lived-in install) used to block the event loop at every submit; they
    // now run in the turn task (`prepare_turn`).
    let stash = rt
        .history
        .push_before_turn(&rt.session.messages, project_dir);
    if submit_images.is_empty() {
        rt.session.add_user_message(&expanded);
    } else {
        match crate::images::build_user_blocks(&expanded, submit_images) {
            Ok(blocks) => rt.session.add_user_message_blocks(blocks),
            Err(e) => {
                app.toasts.push(
                    crate::toast::ToastKind::Warning,
                    format!("Image attach failed: {e}"),
                );
                if expanded.trim().is_empty() {
                    rt.session
                        .add_user_message(&format!("(failed to load image: {e})"));
                } else {
                    rt.session.add_user_message(&expanded);
                }
            }
        }
    }
    if config.session.auto_title {
        let seed = rt
            .session
            .first_user_text()
            .unwrap_or_else(|| expanded.clone());
        if rt.session.apply_heuristic_title(&seed) {
            app.session_title = rt.session.title.clone();
        }
    }
    (expanded, stash)
}

/// Submit-time work moved off the event loop, run in the turn task before
/// the first LLM step. Memory recall must be in place before the request;
/// the undo stash must finish before any tool can edit a file. They run
/// side by side.
pub(super) async fn prepare_turn(
    agent: &Agent,
    session: &mut Session,
    stash: whycodes_session::StashSlot,
    project_dir: &std::path::Path,
    config: &Config,
    query: &str,
) {
    let stash_done = tokio::task::spawn_blocking(move || stash.wait());
    refresh_session_memory(session, agent, project_dir, config, Some(query));
    note_stash_join(stash_done.await);
}

pub(super) fn route_turn_model(
    session_id: &str,
    provider: &str,
    model: &str,
    expanded: &str,
    fast: Option<&str>,
) -> (String, String) {
    let (route_provider, route_model) =
        whycodes_agent::resolve_turn_model(provider, model, expanded, fast);
    if route_model != model || route_provider != provider {
        tracing::info!(
            from = %format!("{provider}/{model}"),
            to = %format!("{route_provider}/{route_model}"),
            "routed trivial turn to fast model"
        );
        whycodes_core::logging::emit_sid(
            "tui",
            "info",
            "turn.route_fast",
            Some(session_id),
            Some(serde_json::json!({
                "from": format!("{provider}/{model}"),
                "to": format!("{route_provider}/{route_model}"),
            })),
        );
    }
    (route_provider, route_model)
}

pub(super) fn should_force_stop(
    agent_busy: bool,
    cancel_requested_at: Option<Instant>,
    pending_cancel: bool,
) -> bool {
    agent_busy
        && cancel_requested_at
            .map(|since| since.elapsed() >= CANCEL_FORCE_AFTER || pending_cancel)
            .unwrap_or(false)
}

/// In-loop hard stop: abort a wedged turn after [`CANCEL_FORCE_AFTER`] or a
/// second `[stop]` while already cancelling.
pub(super) fn maybe_force_stop_in_loop(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    cancel_requested_at: &mut Option<Instant>,
    config: &Config,
    project_dir: &std::path::Path,
    file_index: &Arc<whycodes_index::WorkspaceIndex>,
) {
    if !should_force_stop(rt.agent_busy, *cancel_requested_at, app.pending_cancel) {
        return;
    }
    app.pending_cancel = false;
    force_stop_turn(
        app,
        rt,
        cancel_requested_at,
        config,
        project_dir,
        file_index,
    );
}

/// Local in-process agent turn. Records the user message, routes the model,
/// and delivers [`TurnOutcome::Ok`] / [`TurnOutcome::Err`].
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_local_turn(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    cancel_requested_at: &mut Option<Instant>,
    prompt: &str,
    submit_images: &[crate::images::PromptImage],
    project_dir: &std::path::Path,
    config: &Config,
    provider: &str,
    model: &str,
    api_key: &str,
    max_turns: Option<usize>,
    title_tx: mpsc::UnboundedSender<(String, String)>,
) {
    let flag = arm_generating(app, rt, cancel_requested_at, "");
    let (expanded, stash) = record_user_turn(app, rt, prompt, project_dir, config, submit_images);
    let (route_provider, route_model) = route_turn_model(
        rt.session.id.as_str(),
        provider,
        model,
        &expanded,
        rt.agent
            .model_fast()
            .or(config.session.model_fast.as_deref()),
    );

    let provider2 = route_provider;
    let model2 = route_model;
    let api_key2 = api_key.to_string();
    let event_tx2 = rt.event_tx.clone();
    let done_tx2 = rt.done_tx.clone();
    let cancel2 = Some(flag);
    let auto_title = config.session.auto_title;
    let title_model = config.session.title_model.clone();
    let title_tx2 = title_tx;
    let title_provider = provider.to_string();
    let title_session_model = model.to_string();
    let prep_dir = project_dir.to_path_buf();
    let prep_config = config.clone();

    rt.steer = rt.agent.steer_inbox();
    rt.worker_cancels = rt.agent.worker_cancels();
    rt.steer_session = rt.session.id.clone();
    rt.steered.clear();
    let (ag, sess) = take_turn_owner(rt, project_dir);

    rt.turn_join = Some(tokio::spawn(async move {
        let agent = ag;
        let mut session = sess;
        prepare_turn(
            &agent,
            &mut session,
            stash,
            &prep_dir,
            &prep_config,
            &expanded,
        )
        .await;
        // Time only the agent loop. Title refine runs async *after*
        // we release rt.agent_busy so the user can type immediately.
        let work_t0 = Instant::now();
        let result = agent
            .run_turn_with_events(
                &mut session,
                TurnOpts {
                    provider_name: &provider2,
                    model: &model2,
                    api_key: &api_key2,
                    max_turns,
                    events: Some(event_tx2),
                    cancel: cancel2,
                },
            )
            .await;
        let work_ms = work_t0.elapsed().as_millis();
        // Kick off small-model title refine without awaiting — the
        // main loop applies the title when title_tx delivers.
        if auto_title && result.is_ok() {
            agent.spawn_title_refine(
                &session,
                &title_provider,
                &title_session_model,
                &api_key2,
                title_model.as_deref(),
                title_tx2,
            );
        }
        match result {
            Ok(text) => {
                let _ = done_tx2.send(TurnOutcome::Ok {
                    text,
                    agent,
                    session,
                    work_ms,
                });
            }
            Err(e) => {
                let msg = e.to_string();
                let cancelled = msg.to_ascii_lowercase().contains("cancel");
                let _ = done_tx2.send(TurnOutcome::Err {
                    error: msg,
                    agent,
                    session,
                    cancelled,
                    work_ms,
                });
            }
        }
    }));
}

/// Remote `whycodes serve` turn: stream over HTTP, deliver [`TurnOutcome::Remote`].
pub(super) fn spawn_remote_turn(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    cancel_requested_at: &mut Option<Instant>,
    rem: crate::remote::RemoteAttach,
    prompt: &str,
    project_dir: &std::path::Path,
) {
    let expanded = expand_at_files(prompt, project_dir);
    rt.session.add_user_message(&expanded);
    let flag = arm_generating(app, rt, cancel_requested_at, "remote…");
    let event_tx2 = rt.event_tx.clone();
    let done_tx2 = rt.done_tx.clone();
    rt.turn_join = Some(tokio::spawn(async move {
        let t0 = Instant::now();
        let result = crate::remote::stream_chat(&rem, &expanded, event_tx2, Some(flag)).await;
        let work_ms = t0.elapsed().as_millis();
        match result {
            Ok(text) => {
                if let Err(e) = done_tx2.send(TurnOutcome::Remote {
                    text,
                    error: None,
                    work_ms,
                }) {
                    tracing::debug!(error = %e, "remote turn done dropped");
                }
            }
            Err(e) => {
                if let Err(send_err) = done_tx2.send(TurnOutcome::Remote {
                    text: String::new(),
                    error: Some(e.to_string()),
                    work_ms,
                }) {
                    tracing::debug!(error = %send_err, "remote turn err dropped");
                }
            }
        }
    }));
}

/// Spawn `/compact` off the event loop (as a running command). The pager
/// keeps painting and Esc still force-stops after [`CANCEL_FORCE_AFTER`].
#[allow(clippy::too_many_arguments)]
pub(super) fn start_compact_task(
    app: &mut TuiApp,
    rt: &mut SessionRuntime,
    cancel_requested_at: &mut Option<Instant>,
    note: String,
    provider: &str,
    model: &str,
    api_key: &str,
    project_dir: &std::path::Path,
) {
    rt.agent_busy = true;
    let flag = new_cancel_flag();
    rt.cancel_flag = Some(Arc::clone(&flag));
    *cancel_requested_at = None;
    app.current_agent_state = AgentState::Generating;
    app.status_message = "Compacting conversation…".into();
    app.mark_dirty();

    let (ag, sess) = take_turn_owner(rt, project_dir);
    let provider2 = provider.to_string();
    let model2 = model.to_string();
    let api_key2 = api_key.to_string();
    let done_tx2 = rt.done_tx.clone();
    let user_context = if note.is_empty() { None } else { Some(note) };
    rt.turn_join = Some(tokio::spawn(async move {
        let t0 = Instant::now();
        let agent = ag;
        let mut session = sess;
        let outcome = agent
            .compact_session(
                &mut session,
                &provider2,
                &model2,
                &api_key2,
                user_context.as_deref(),
            )
            .await;
        let work_ms = t0.elapsed().as_millis();
        let _ = done_tx2.send(TurnOutcome::Compact {
            agent,
            session,
            outcome,
            work_ms,
        });
    }));
}

pub(super) fn take_turn_owner(
    rt: &mut SessionRuntime,
    project_dir: &std::path::Path,
) -> (Agent, Session) {
    // The placeholder shares the session's job registry: quitting or closing
    // the session mid-turn kills its jobs through `rt.agent`, and a fresh
    // registry there left them running.
    let jobs = rt.agent.background_registry().clone();
    // It keeps the session's tools too: Ctrl+N mid-turn builds the new
    // session from `rt.agent.shared_tools()`, and an empty executor there
    // left it without MCP servers or plugins.
    let tools = rt.agent.shared_tools();
    let ag = std::mem::replace(
        &mut rt.agent,
        Agent::new(whycodes_core::types::AgentInfo {
            name: "_pending".into(),
            description: String::new(),
            mode: AgentMode::Primary,
            permission: whycodes_core::types::PermissionSet::default(),
            model: None,
            system_prompt: Some(String::new()),
            temperature: None,
            top_p: None,
        })
        .with_background_registry(jobs)
        .with_shared_tools(tools),
    );
    rt.session_backup = Some(rt.session.clone());
    let sess = std::mem::replace(
        &mut rt.session,
        Session::new(project_dir.to_path_buf(), String::new()),
    );
    (ag, sess)
}

pub(super) fn apply_turn_event(app: &mut TuiApp, ev: TurnEvent) {
    match ev {
        TurnEvent::TextDelta(t) => {
            app.finish_open_thinking();
            app.current_agent_state = AgentState::Generating;
            app.append_to_last(&t);
        }
        TurnEvent::ThinkingDelta(t) => {
            app.current_agent_state = AgentState::Thinking;
            app.append_thinking(&t);
        }
        TurnEvent::DraftDiscarded { text, thinking } => app.discard_draft(&text, &thinking),
        TurnEvent::ToolStart { id, name, input } => {
            app.finish_open_thinking();
            // Short labels in the busy strip (`bash` → `run`).
            app.status_message = format!("tool: {}", shown_tool_name(&name));
            if matches!(name.as_str(), "todowrite" | "todo")
                && let Some(next) = whycodes_core::todo::apply_todowrite_args(&app.todos, &input)
            {
                app.replace_todos(next);
            }
            app.add_tool_call(id, name, input);
        }
        TurnEvent::ToolEnd {
            id,
            content,
            is_error,
        } => {
            app.add_tool_result(&id, content, is_error);
        }
        TurnEvent::Status(s) => {
            // Post-turn niceties (e.g. async memory retain) may arrive after
            // Idle — surface as a quiet toast so we don't clobber "Worked for…".
            if !app.is_busy() && s.starts_with("Remembered ") {
                app.toasts
                    .push(crate::toast::ToastKind::Info, truncate_toast(&s, 48));
            } else {
                app.status_message = s;
            }
            app.mark_dirty();
        }
        TurnEvent::Intent {
            kind,
            confidence: _,
            badge,
            notice_kind,
            notice,
        } => {
            app.intent_kind = Some(kind);
            app.intent_badge = if badge.is_empty() { None } else { Some(badge) };
            if !notice.is_empty() {
                let toast_kind = match notice_kind.as_str() {
                    "warning" => crate::toast::ToastKind::Warning,
                    _ => crate::toast::ToastKind::Info,
                };
                // Warnings: full message (mode mismatch). Info: compact.
                let msg = if matches!(toast_kind, crate::toast::ToastKind::Warning) {
                    truncate_toast(&notice, 96)
                } else {
                    truncate_toast(&notice, 56)
                };
                app.toasts.push(toast_kind, msg);
            }
            app.mark_dirty();
        }
        TurnEvent::Usage(usage) => {
            app.stream_speed.close_step(usage.output_tokens);
            app.turn_usage = Some(usage.clone());
            // Per-step input size ≈ context window fill (meter).
            app.set_context_from_usage(&usage);
            let mut parts = Vec::new();
            if let Some(ms) = app.turn_elapsed_ms() {
                parts.push(format_elapsed_ms(ms));
            }
            parts.push(format_usage_short(&usage));
            app.status_message = parts.join(" · ");
            app.mark_dirty();
        }
        TurnEvent::Cancelled => {
            app.finish_open_thinking();
            app.status_message = "Cancelled.".into();
            app.current_agent_state = AgentState::Idle;
            app.mark_dirty();
        }
        TurnEvent::FileConflict {
            path,
            claimant,
            owner,
        } => {
            // Conflict notify: short warning toast so concurrent writers are visible.
            let short_path = path.rsplit('/').next().unwrap_or(&path);
            app.toasts.push(
                crate::toast::ToastKind::Warning,
                truncate_toast(
                    &format!("File conflict: {short_path} ({claimant} vs {owner})"),
                    72,
                ),
            );
            app.status_message = format!("conflict: {short_path}");
            app.mark_dirty();
        }
        TurnEvent::SwarmStatus {
            active: _,
            total,
            message,
        } => {
            app.status_message = if message.is_empty() {
                format!("swarm {total}…")
            } else {
                message
            };
            app.mark_dirty();
        }
        TurnEvent::Background {
            id,
            status,
            summary,
        } => {
            apply_background_event(app, &id, &status, &summary);
            app.mark_dirty();
        }
        TurnEvent::EnqueuePrompt { text } => {
            if !text.trim().is_empty() {
                app.pending_auto_prompts.push_back(text);
                app.toasts.push(
                    crate::toast::ToastKind::Info,
                    truncate_toast(
                        &format!("queued · {} left", app.pending_auto_prompts.len()),
                        40,
                    ),
                );
                app.mark_dirty();
            }
        }
        TurnEvent::Panel(update) => {
            apply_panel_update(app, update);
        }
        TurnEvent::Todos { todos } => {
            app.replace_todos(todos);
        }
        TurnEvent::Subagent {
            id,
            kind,
            description,
            status,
            activity,
            elapsed_ms,
            output,
            tokens,
        } => {
            let key = id.clone();
            app.upsert_subagent(crate::app::SubagentUpdate {
                id,
                kind,
                description,
                status,
                activity,
                elapsed_ms,
                output,
            });
            app.set_subagent_tokens(&key, tokens);
        }
        TurnEvent::SwarmMessage { from, to, text } => {
            app.toasts.push(
                crate::toast::ToastKind::Info,
                truncate_toast(&format!("swarm {from}→{to}: {text}"), 72),
            );
            app.mark_dirty();
        }
        TurnEvent::PermissionAsk { .. } => {}
        TurnEvent::QuestionAsk { .. } => {}
        TurnEvent::FileStale {
            path,
            reader,
            writer,
        } => {
            let short = path.rsplit('/').next().unwrap_or(&path);
            app.toasts.push(
                crate::toast::ToastKind::Warning,
                truncate_toast(&format!("stale read: {short} ({reader} vs {writer})"), 72),
            );
            app.mark_dirty();
        }
    }
}

pub(super) fn apply_background_event(app: &mut TuiApp, id: &str, status: &str, summary: &str) {
    // A `bash` call that finished before it was detached still reports its
    // end. It never ran in the background (no "running" was ever sent), so
    // it gets no toast and no task row.
    if matches!(status, "done" | "failed" | "killed") && !app.bg_jobs.iter().any(|j| j.id == id) {
        return;
    }
    match status {
        "running" => {
            app.bg_running_count = app.bg_running_count.saturating_add(1);
            app.upsert_bg_job(id, "running", summary);
            app.status_message = format!("bg {id} started");
            app.toasts.push(
                crate::toast::ToastKind::Info,
                truncate_toast(&format!("bg {id}: {summary}"), 56),
            );
        }
        "done" => {
            app.bg_running_count = app.bg_running_count.saturating_sub(1);
            app.upsert_bg_job(id, "done", summary);
            app.toasts.push(
                crate::toast::ToastKind::Success,
                truncate_toast(&format!("bg {id} done · {summary}"), 56),
            );
        }
        "failed" => {
            app.bg_running_count = app.bg_running_count.saturating_sub(1);
            app.upsert_bg_job(id, "failed", summary);
            app.toasts.push(
                crate::toast::ToastKind::Warning,
                truncate_toast(&format!("bg {id} failed · {summary}"), 64),
            );
        }
        "killed" => {
            app.bg_running_count = app.bg_running_count.saturating_sub(1);
            app.upsert_bg_job(id, "killed", summary);
            app.toasts.push(
                crate::toast::ToastKind::Info,
                truncate_toast(&format!("bg {id} killed"), 40),
            );
        }
        _ => {
            app.upsert_bg_job(id, status, summary);
            app.status_message = format!("bg {id} {status}");
        }
    }
}

/// Say so when a turn ends with no answer text (only thinking or tool calls):
/// otherwise the transcript just stops after "Worked for…".
pub(super) fn note_empty_answer(app: &mut TuiApp, text: &str) {
    if text.trim().is_empty() {
        app.add_message(ChatRole::System, EMPTY_ANSWER_NOTE);
    }
}
