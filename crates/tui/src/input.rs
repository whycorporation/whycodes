// ── input.rs: Input handler ────────────────────────────────────────────
// Translates raw crossterm events into actions via the keymap,
// then updates application state accordingly.
//
// Focus model (Grok Build): Prompt vs Scrollback own the keyboard.
// Esc hierarchy: overlays → cancel turn (run loop) → double-Esc clear draft.

use crate::app::{
    AppMode, ConfirmAction, DialogKind, ESC_DOUBLE_MS, FocusPane, MouseSelection, TuiApp,
};
use crate::keymap::{Action, KeymapContext};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind,
};
use std::time::Instant;

/// Process a single crossterm event and update application state.
/// Returns `false` when the app should exit.
pub fn handle_event(app: &mut TuiApp, event: Event) -> bool {
    match event {
        Event::Key(key) => handle_key(app, key),
        Event::Mouse(mouse) => handle_mouse(app, mouse),
        Event::Paste(data) => {
            handle_paste(app, &data);
            true
        }
        Event::Resize(_, _) => {
            // Viewport changed (window snap, phone rotate, OSK show/hide).
            // Force a full paint so layout is not left on the previous size.
            app.mark_dirty();
            true
        }
        _ => true,
    }
}

/// Bracketed paste / drag-drop: image paths become attachments; other text
/// inserts at the cursor. Dragging a file onto most terminals pastes its path.
fn handle_paste(app: &mut TuiApp, data: &str) {
    // Only while the prompt can accept input.
    if app.mode != AppMode::Normal && app.mode != AppMode::Session {
        return;
    }
    if app.focus != FocusPane::Prompt && app.mode != AppMode::Normal {
        // Still allow paste to land on the prompt (common when focus drifted).
    }
    app.focus_prompt();

    let classified = crate::images::classify_paste(data);
    let mut attached = 0usize;
    let mut last_err: Option<String> = None;
    for path in classified.images {
        match app.attach_image(&path) {
            Ok(()) => attached += 1,
            Err(e) => last_err = Some(e),
        }
    }
    if attached > 0 {
        let label = if attached == 1 {
            app.pending_images
                .last()
                .map(|i| format!("Attached {}", i.label))
                .unwrap_or_else(|| "Image attached".into())
        } else {
            format!("Attached {attached} images")
        };
        app.toasts.push(crate::toast::ToastKind::Success, label);
    }
    if let Some(err) = last_err {
        app.toasts.push(crate::toast::ToastKind::Warning, err);
    }

    let text = classified.text;
    if text.is_empty() {
        return;
    }
    // Large pastes collapse to `[pasted #N ~ L lines]` so the
    // prompt does not reflow/flicker; full body is restored on submit.
    app.insert_paste_text(&text);
    app.slash_suggest.refresh(&app.input_buffer);
    app.file_suggest
        .refresh(&app.input_buffer, app.input_cursor);
    app.esc_armed_at = None;
}

fn handle_key(app: &mut TuiApp, key: KeyEvent) -> bool {
    // Windows (and enhanced keyboard) emits Press + Release for every key.
    // Without this filter each character is inserted twice.
    if key.kind != KeyEventKind::Press && key.kind != KeyEventKind::Repeat {
        return true;
    }

    let ctx = app.key_context;

    if app.open_subagent.is_some()
        && matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
        && !key
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL)
    {
        app.close_subagent_view();
        return true;
    }

    // If dialog mode, route to dialog handler first.
    if ctx == KeymapContext::Dialog {
        return handle_dialog_key(app, &key);
    }
    // Cheatsheet owns `/` search and Esc-to-clear before the keymap.
    if ctx == KeymapContext::Help && handle_help_type(app, &key) {
        app.mark_dirty();
        return true;
    }

    // Resolve and dispatch (focus-aware).
    let action = crate::keymap::Keymap::new().resolve(ctx, app.focus, &key);

    match action {
        Some(Action::Quit) => {
            if app.mode != AppMode::Command && app.mode != AppMode::Dialog {
                app.confirm(
                    "Quit",
                    "Are you sure you want to quit whycode?",
                    ConfirmAction::Quit,
                );
            }
            true
        }
        Some(Action::EnterCommand) => {
            if app.mode == AppMode::Normal {
                app.mode = AppMode::Command;
                app.key_context = KeymapContext::Command;
                app.command.buffer = String::from(":");
            }
            true
        }
        Some(Action::EscapeMode) => {
            handle_escape(app);
            true
        }
        // `@file` picker: Tab drills into dirs / accepts files, Enter accepts,
        // arrows navigate. These guards precede the slash-suggest ones; the
        // two popups are mutually exclusive by construction.
        Some(Action::ToggleFocus) if app.file_suggest.active => {
            app.file_suggest
                .accept(&mut app.input_buffer, &mut app.input_cursor);
            app.mark_dirty();
            true
        }
        Some(Action::SubmitInput) if app.file_suggest.active => {
            if app.file_suggest.matches.is_empty() {
                app.file_suggest.dismiss();
            } else {
                app.file_suggest
                    .accept(&mut app.input_buffer, &mut app.input_cursor);
            }
            app.mark_dirty();
            true
        }
        Some(Action::InputHistoryPrev) if app.file_suggest.active => {
            app.file_suggest.step(-1);
            true
        }
        Some(Action::InputHistoryNext) if app.file_suggest.active => {
            app.file_suggest.step(1);
            true
        }
        Some(Action::FileComplete) => {
            app.focus_prompt();
            app.file_suggest
                .activate(&mut app.input_buffer, &mut app.input_cursor);
            app.mark_dirty();
            true
        }
        // Slash-suggest: Tab completes; Up/Down navigate the list.
        Some(Action::ToggleFocus)
            if app.pending_suggestion.is_some() && app.input_buffer.trim().is_empty() =>
        {
            if let Some(s) = app.pending_suggestion.take() {
                app.input_buffer = s;
                app.input_cursor = app.input_buffer.len();
                app.status_message = "suggestion accepted".into();
                app.mark_dirty();
            }
            true
        }
        Some(Action::ToggleFocus) if app.slash_suggest.active => {
            if let Some(cmd) = app.slash_suggest.current()
                && cmd.name != app.input_buffer
            {
                app.input_buffer = cmd.name.to_string();
                app.input_cursor = app.input_buffer.len();
                app.slash_suggest.refresh(&app.input_buffer);
            }
            true
        }
        Some(Action::SelectPrev) if app.slash_suggest.active => {
            app.slash_suggest.step(-1);
            true
        }
        Some(Action::SelectNext) if app.slash_suggest.active => {
            app.slash_suggest.step(1);
            true
        }
        Some(Action::InputHistoryPrev) if app.slash_suggest.active => {
            app.slash_suggest.step(-1);
            true
        }
        Some(Action::InputHistoryNext) if app.slash_suggest.active => {
            app.slash_suggest.step(1);
            true
        }
        Some(Action::ToggleFocus) => {
            app.toggle_focus();
            true
        }
        Some(Action::FocusPrompt) if selected_subagent_id(app).is_some() => {
            open_selected_subagent(app);
            true
        }
        Some(Action::FocusPrompt) => {
            app.focus_prompt();
            true
        }
        Some(Action::FocusScrollback) => {
            app.focus_scrollback();
            true
        }
        Some(Action::SelectPrev) => {
            app.move_selection(-1);
            true
        }
        Some(Action::SelectNext) => {
            app.move_selection(1);
            true
        }
        Some(Action::JumpPrevTurn) => {
            app.jump_user_turn(false);
            true
        }
        Some(Action::JumpNextTurn) => {
            app.jump_user_turn(true);
            true
        }
        Some(Action::CopySelection) => {
            app.copy_selected_message();
            true
        }
        Some(Action::ToggleThinking) => {
            app.toggle_selected_thinking();
            true
        }
        Some(Action::ToggleToolResult) => {
            app.toggle_selected_tools();
            true
        }
        Some(Action::SubmitInput) => {
            app.slash_suggest.dismiss();
            match app.mode {
                AppMode::Normal => {
                    app.submit_input();
                }
                AppMode::Command => {
                    let cmd = app.command.buffer.clone();
                    app.command.buffer.clear();
                    app.mode = AppMode::Normal;
                    app.key_context = KeymapContext::Normal;
                    execute_command(app, &cmd);
                }
                _ => {}
            }
            true
        }
        Some(Action::ToggleSidebar) => {
            app.sidebar.visible = !app.sidebar.visible;
            app.mark_dirty();
            true
        }
        Some(Action::ToggleTasksPane) => {
            app.toggle_tasks_pane();
            true
        }
        Some(Action::OpenSubagent) => {
            open_selected_subagent(app);
            true
        }
        Some(Action::SidebarNextTab) => {
            if app.sidebar.visible {
                app.sidebar.active_tab = app.sidebar.active_tab.next();
                app.mark_dirty();
            }
            true
        }
        Some(Action::SidebarPrevTab) => {
            if app.sidebar.visible {
                app.sidebar.active_tab = app.sidebar.active_tab.prev();
                app.mark_dirty();
            }
            true
        }
        Some(Action::OpenProviderDialog) => {
            open_provider_dialog(app);
            true
        }
        Some(Action::OpenModelDialog) => {
            open_model_dialog(app);
            true
        }
        Some(Action::ToggleAutoScroll) => {
            app.auto_scroll = !app.auto_scroll;
            app.status_message = if app.auto_scroll {
                "Auto-scroll ON".into()
            } else {
                "Auto-scroll OFF".into()
            };
            true
        }
        Some(Action::ClearSession) => {
            app.confirm(
                "Clear Session",
                "Clear all messages from this session?",
                ConfirmAction::ClearSession,
            );
            true
        }
        Some(Action::SwitchAgent) => {
            // Handled in run loop (needs Agent + Config).
            app.status_message = "Ctrl+T: switch agent (run loop)".into();
            true
        }
        Some(Action::ScrollUp) => {
            app.scroll_rows(-1);
            true
        }
        Some(Action::ScrollDown) => {
            app.scroll_rows(1);
            true
        }
        Some(Action::ScrollPageUp) => {
            app.scroll_page(true);
            true
        }
        Some(Action::ScrollPageDown) => {
            app.scroll_page(false);
            true
        }
        Some(Action::ScrollToTop) => {
            app.scroll_to_top();
            true
        }
        Some(Action::ScrollToBottom) => {
            app.scroll_to_bottom();
            true
        }
        // Input editing actions
        Some(action) => {
            // Typing implies prompt focus.
            if matches!(
                action,
                Action::InputBackspace
                    | Action::InputDelete
                    | Action::InputLeft
                    | Action::InputRight
                    | Action::InputHome
                    | Action::InputEnd
                    | Action::InputNewline
                    | Action::InputClear
                    | Action::InputHistoryPrev
                    | Action::InputHistoryNext
            ) {
                app.focus = FocusPane::Prompt;
            }
            handle_input_action(app, action, &key);
            true
        }
        None => {
            // Unmapped key — treat as text input. Grok simple mode: any letter
            // while scrollback is focused auto-focuses the prompt.
            // Stale key_context (still Normal) still has to close help on `q`.
            if app.mode == AppMode::Help
                && !handle_help_type(app, &key)
                && matches!(key.code, KeyCode::Char('q'))
            {
                app.mode = AppMode::Normal;
                app.key_context = KeymapContext::Normal;
                return true;
            }
            match app.mode {
                AppMode::Normal => {
                    if let KeyCode::Char(c) = key.code {
                        app.focus_prompt();
                        // Second `/` while the draft is already a bare slash
                        // (typical after Esc dismisses the popup but leaves `/`)
                        // reopens the menu instead of turning the buffer into
                        // `//`, which matches no command names.
                        if c == '/' && is_bare_slash_draft(&app.input_buffer) {
                            app.input_buffer = "/".to_string();
                            app.input_cursor = 1;
                            app.slash_suggest.refresh(&app.input_buffer);
                            app.esc_armed_at = None;
                            return true;
                        }
                        let pos = clamp_cursor(&app.input_buffer, app.input_cursor);
                        app.input_buffer.insert(pos, c);
                        app.input_cursor = pos + c.len_utf8();
                        app.slash_suggest.refresh(&app.input_buffer);
                        app.file_suggest
                            .refresh(&app.input_buffer, app.input_cursor);
                        app.esc_armed_at = None;
                    }
                }
                AppMode::Command => {
                    if let KeyCode::Char(c) = key.code {
                        app.command.buffer.push(c);
                    }
                }
                _ => {}
            }
            true
        }
    }
}

/// True when the prompt holds only a bare `/` draft (optionally more slashes).
/// Used so a second `/` reopens the command menu instead of building `//`.
fn is_bare_slash_draft(buf: &str) -> bool {
    !buf.is_empty() && buf.bytes().all(|b| b == b'/')
}

/// Grok Esc hierarchy (steal-Esc first, then double-Esc clear).
///
/// Cancel-while-busy is owned by the run loop (has the CancelFlag); here we
/// only handle idle clear / mode exit. Slash + help still steal Esc.
fn selected_subagent_id(app: &TuiApp) -> Option<String> {
    let idx = app.selected_msg?;
    let msg = app.messages.get(idx)?;
    for block in &msg.blocks {
        if let crate::app::ChatBlock::Subagent { id, .. } = block {
            return Some(id.clone());
        }
    }
    None
}

fn open_selected_subagent(app: &mut TuiApp) {
    if let Some(id) = selected_subagent_id(app) {
        app.open_subagent_view(&id);
    } else if let Some(row) = app.subagents.last() {
        let id = row.id.clone();
        app.open_subagent_view(&id);
    }
}

fn handle_escape(app: &mut TuiApp) {
    if app.open_subagent.is_some() {
        app.close_subagent_view();
        return;
    }
    // 0. Steal: an in-flight OAuth paste-code login is cancelled first.
    if app.auth_code_sink.take().is_some() {
        app.status_message = "sign-in cancelled".into();
        app.mark_dirty();
        return;
    }
    // 1. Steal: slash suggest
    if app.slash_suggest.active {
        app.slash_suggest.dismiss();
        // Drop a lone `/` so the next `/` opens a clean menu (otherwise the
        // buffer stays `/` and a second press becomes `//` with no matches).
        if is_bare_slash_draft(&app.input_buffer) {
            app.input_buffer.clear();
            app.input_cursor = 0;
        }
        app.esc_armed_at = None;
        return;
    }
    // 1b. Steal: `@file` picker (leave the typed `@token` as-is).
    if app.file_suggest.active {
        app.file_suggest.dismiss();
        app.esc_armed_at = None;
        return;
    }
    match app.mode {
        AppMode::Command => {
            app.mode = AppMode::Normal;
            app.key_context = KeymapContext::Normal;
            app.command.buffer.clear();
            app.esc_armed_at = None;
            return;
        }
        AppMode::Help => {
            if app.help_searching || !app.help_query.is_empty() {
                app.help_query.clear();
                app.help_searching = false;
                app.help_scroll = 0;
                app.esc_armed_at = None;
                return;
            }
            app.mode = AppMode::Normal;
            app.key_context = KeymapContext::Normal;
            app.esc_armed_at = None;
            return;
        }
        _ => {}
    }

    // Busy cancel is handled in run.rs before this path; if we still get here
    // while busy, do not clear the draft (Grok: Esc preserves draft on cancel).
    if app.is_busy() {
        app.esc_armed_at = None;
        return;
    }

    // Double-Esc clear when prompt has a draft or staged images (Grok: 800ms).
    if app.prompt_has_content() {
        let now = Instant::now();
        if let Some(armed) = app.esc_armed_at
            && now.duration_since(armed).as_millis() <= ESC_DOUBLE_MS
        {
            app.clear_prompt_draft();
            app.toasts
                .push(crate::toast::ToastKind::Info, "Prompt cleared");
            return;
        }
        app.esc_armed_at = Some(now);
        app.toasts
            .push(crate::toast::ToastKind::Info, "press Esc again to clear");
        return;
    }

    // Empty prompt + scrollback focused → return to prompt
    if app.focus == FocusPane::Scrollback {
        app.focus_prompt();
        app.esc_armed_at = None;
        return;
    }

    app.esc_armed_at = None;
}

fn handle_input_action(app: &mut TuiApp, action: Action, _key: &KeyEvent) {
    match action {
        Action::InputBackspace if app.input_cursor > 0 => {
            let end = clamp_cursor(&app.input_buffer, app.input_cursor);
            // Collapsed paste tokens delete as a unit (like a single grapheme).
            if let Some(span) = crate::paste::placeholder_ending_at(&app.input_buffer, end)
                .or_else(|| crate::paste::placeholder_at(&app.input_buffer, end.saturating_sub(1)))
            {
                app.remove_paste_span(span.start, span.end, span.id);
            } else {
                let start = prev_boundary(&app.input_buffer, end);
                app.input_buffer.replace_range(start..end, "");
                app.input_cursor = start;
                crate::paste::prune_unused(&mut app.pending_pastes, &app.input_buffer);
            }
            app.slash_suggest.refresh(&app.input_buffer);
            app.file_suggest
                .refresh(&app.input_buffer, app.input_cursor);
        }
        // Empty buffer + staged images: Backspace peels off the last attachment.
        Action::InputBackspace
            if app.input_cursor == 0
                && app.input_buffer.is_empty()
                && !app.pending_images.is_empty() =>
        {
            if let Some(img) = app.pop_pending_image() {
                app.toasts.push(
                    crate::toast::ToastKind::Info,
                    format!("Removed {}", img.label),
                );
            }
        }
        Action::InputDelete if app.input_cursor < app.input_buffer.len() => {
            let start = clamp_cursor(&app.input_buffer, app.input_cursor);
            if let Some(span) = crate::paste::placeholder_starting_at(&app.input_buffer, start)
                .or_else(|| crate::paste::placeholder_at(&app.input_buffer, start))
            {
                app.remove_paste_span(span.start, span.end, span.id);
            } else {
                let end = next_boundary(&app.input_buffer, start);
                app.input_buffer.replace_range(start..end, "");
                app.input_cursor = start;
                crate::paste::prune_unused(&mut app.pending_pastes, &app.input_buffer);
            }
            app.slash_suggest.refresh(&app.input_buffer);
            app.file_suggest
                .refresh(&app.input_buffer, app.input_cursor);
        }
        Action::InputClear => {
            app.input_buffer.clear();
            app.input_cursor = 0;
            app.pending_images.clear();
            app.pending_pastes.clear();
            app.slash_suggest.dismiss();
            app.file_suggest.dismiss();
        }
        Action::InputLeft => {
            let pos = clamp_cursor(&app.input_buffer, app.input_cursor);
            if let Some(span) = crate::paste::placeholder_ending_at(&app.input_buffer, pos) {
                app.input_cursor = span.start;
            } else if let Some(span) =
                crate::paste::placeholder_at(&app.input_buffer, pos.saturating_sub(1))
            {
                app.input_cursor = span.start;
            } else {
                app.input_cursor = prev_boundary(&app.input_buffer, app.input_cursor);
            }
            app.file_suggest
                .refresh(&app.input_buffer, app.input_cursor);
        }
        Action::InputRight if app.input_cursor < app.input_buffer.len() => {
            let pos = clamp_cursor(&app.input_buffer, app.input_cursor);
            if let Some(span) = crate::paste::placeholder_starting_at(&app.input_buffer, pos)
                .or_else(|| crate::paste::placeholder_at(&app.input_buffer, pos))
            {
                app.input_cursor = span.end;
            } else {
                app.input_cursor = next_boundary(&app.input_buffer, app.input_cursor);
            }
            app.file_suggest
                .refresh(&app.input_buffer, app.input_cursor);
        }
        Action::InputHome => {
            app.input_cursor = 0;
            app.file_suggest
                .refresh(&app.input_buffer, app.input_cursor);
        }
        Action::InputEnd => {
            app.input_cursor = app.input_buffer.len();
            app.file_suggest
                .refresh(&app.input_buffer, app.input_cursor);
        }
        Action::InputNewline => {
            let pos = clamp_cursor(&app.input_buffer, app.input_cursor);
            app.input_buffer.insert(pos, '\n');
            app.input_cursor = pos + 1;
            app.slash_suggest.dismiss();
            app.file_suggest.dismiss();
        }
        Action::InputHistoryPrev if !app.input_history.is_empty() && app.input_history_idx > 0 => {
            app.input_history_idx -= 1;
            // History stores expanded text — no live paste blocks.
            app.pending_pastes.clear();
            app.input_buffer = app.input_history[app.input_history_idx].clone();
            app.input_cursor = app.input_buffer.len();
        }
        Action::InputHistoryNext if app.input_history_idx < app.input_history.len() => {
            app.input_history_idx += 1;
            app.pending_pastes.clear();
            if app.input_history_idx < app.input_history.len() {
                app.input_buffer = app.input_history[app.input_history_idx].clone();
            } else {
                app.input_buffer.clear();
            }
            app.input_cursor = app.input_buffer.len();
        }
        _ => {}
    }
}

fn clamp_cursor(s: &str, idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    if s.is_char_boundary(idx) {
        idx
    } else {
        prev_boundary(s, idx)
    }
}

fn prev_boundary(s: &str, idx: usize) -> usize {
    if idx == 0 {
        return 0;
    }
    let mut i = idx.min(s.len()).saturating_sub(1);
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn next_boundary(s: &str, idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    let mut i = idx + 1;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

fn handle_mouse(app: &mut TuiApp, mouse: MouseEvent) -> bool {
    // Always track pointer for hover chrome (context % meter, etc.).
    //
    // IMPORTANT: this function's return value is "keep running?" for the
    // event loop (`false` = quit the app). Never return `false` for ordinary
    // mouse motion — that was killing the TUI on the first mouse-move after
    // EnableMouseCapture (seen as `tui.exit reason=handle_event=false`).
    //
    // Sticky hover (Grok HitArea): mouse_pos first, then update all chrome
    // hits against the *previous* frame’s rects.
    app.mouse_pos = Some((mouse.column, mouse.row));
    if app.update_chrome_hover() {
        app.mark_dirty();
    }

    // Any modal (dialog stack or Help overlay): same mouse path — wheel,
    // [✗], list clicks, scrollbar, and text select/copy clipped to the popup.
    if app.modal_is_open() {
        return handle_modal_mouse(app, mouse);
    }

    match mouse.kind {
        // Wheel always moves the transcript (dialog path is handled above).
        // Step ≈ ⅓ viewport so one notch is visible on tall terminals; min 3.
        MouseEventKind::ScrollDown => {
            let step = chat_wheel_step(app);
            app.scroll_rows(-step);
        }
        MouseEventKind::ScrollUp => {
            let step = chat_wheel_step(app);
            app.scroll_rows(step);
        }
        MouseEventKind::Moved => {
            return true;
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(id) = app
                .subagent_strip_hit
                .iter()
                .find(|(r, _)| {
                    mouse.column >= r.x
                        && mouse.column < r.x.saturating_add(r.width)
                        && mouse.row >= r.y
                        && mouse.row < r.y.saturating_add(r.height)
                })
                .map(|(_, id)| id.clone())
            {
                app.open_subagent_view(&id);
                app.mouse_sel = None;
                return true;
            }
            // Turn-status [stop] → cancel current agent turn.
            if app.turn_stop_hit.contains(mouse.column, mouse.row) && app.is_busy() {
                app.pending_cancel = true;
                app.status_message = "Cancelling…".into();
                app.mark_dirty();
                app.mouse_sel = None;
                return true;
            }
            // Prompt footer: click agent / model name to open the picker.
            if app.agent_hit.contains(mouse.column, mouse.row) {
                open_agent_dialog(app);
                app.mouse_sel = None;
                return true;
            }
            if app.model_hit.contains(mouse.column, mouse.row) {
                open_model_dialog(app);
                app.mouse_sel = None;
                return true;
            }
            if app.effort_hit.contains(mouse.column, mouse.row) {
                open_effort_dialog(app);
                app.mouse_sel = None;
                return true;
            }
            // Slash dropdown: click a row to select + apply.
            if app.slash_suggest.active
                && let Some(idx) = app.slash_suggest.row_index_at(mouse.column, mouse.row)
            {
                app.slash_suggest.selected = idx;
                if let Some(cmd) = app.slash_suggest.current() {
                    app.input_buffer = format!("{} ", cmd.name);
                    app.input_cursor = app.input_buffer.len();
                    app.slash_suggest.dismiss();
                    app.mark_dirty();
                }
                app.mouse_sel = None;
                return true;
            }
            // File picker: click a row to select + apply (dirs drill down).
            if app.file_suggest.active
                && let Some(idx) = app.file_suggest.row_index_at(mouse.column, mouse.row)
            {
                app.file_suggest.selected = idx;
                app.file_suggest
                    .accept(&mut app.input_buffer, &mut app.input_cursor);
                app.mark_dirty();
                app.mouse_sel = None;
                return true;
            }
            // Chat scrollbar first: thumb drag / track jump (not text select).
            if app.chat_scrollbar_contains(mouse.column, mouse.row)
                && let Some(track) = app.chat_scrollbar_hit
            {
                let grab = chat_scrollbar_grab_at(app, mouse.row, track);
                app.chat_scrollbar_grab = Some(grab);
                apply_chat_scrollbar_offset(app, mouse.row, Some(grab));
                app.mouse_sel = None;
                return true;
            }
            app.chat_scrollbar_grab = None;
            // Start a fresh selection. Shift+drag is left to the terminal for
            // native select (when the host still delivers un-shifted events only).
            app.mouse_sel = Some(MouseSelection {
                anchor_x: mouse.column,
                anchor_y: mouse.row,
                focus_x: mouse.column,
                focus_y: mouse.row,
                dragging: true,
            });
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if app.chat_scrollbar_grab.is_some() {
                let grab = app.chat_scrollbar_grab;
                apply_chat_scrollbar_offset(app, mouse.row, grab);
                return true;
            }
            if let Some(sel) = &mut app.mouse_sel {
                sel.focus_x = mouse.column;
                sel.focus_y = mouse.row;
                sel.dragging = true;
            } else {
                app.mouse_sel = Some(MouseSelection {
                    anchor_x: mouse.column,
                    anchor_y: mouse.row,
                    focus_x: mouse.column,
                    focus_y: mouse.row,
                    dragging: true,
                });
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if app.chat_scrollbar_grab.take().is_some() {
                app.mouse_sel = None;
                app.mark_dirty();
                return true;
            }
            if let Some(sel) = &mut app.mouse_sel {
                sel.focus_x = mouse.column;
                sel.focus_y = mouse.row;
                sel.dragging = false;
                // Linear selection needs the real endpoints, not a normalized
                // rectangle (that would pull in the empty corner of short lines).
                let same_cell = sel.anchor_x == sel.focus_x && sel.anchor_y == sel.focus_y;
                if same_cell {
                    // Plain click (no drag): copy cwd if the path was hit.
                    let col = mouse.column;
                    let row = mouse.row;
                    app.mouse_sel = None;
                    if app.cwd_contains(col, row) {
                        let path = app.project_dir.display().to_string();
                        if crate::clipboard::copy_text(&path) {
                            app.toasts.push(
                                crate::toast::ToastKind::Success,
                                format!("Copied path: {path}"),
                            );
                        } else {
                            app.toasts.push(
                                crate::toast::ToastKind::Warning,
                                "Copy failed — no clipboard",
                            );
                        }
                    }
                } else {
                    let text = crate::clipboard::text_from_cells(
                        &app.screen_cells,
                        sel.anchor_x,
                        sel.anchor_y,
                        sel.focus_x,
                        sel.focus_y,
                    );
                    if text.is_empty() {
                        app.mouse_sel = None;
                    } else if crate::clipboard::copy_text(&text) {
                        app.toasts.push(
                            crate::toast::ToastKind::Info,
                            format!("Copied {} chars", text.chars().count()),
                        );
                    } else {
                        app.toasts.push(
                            crate::toast::ToastKind::Warning,
                            "Copy failed — no clipboard",
                        );
                    }
                }
            }
        }
        _ => {}
    }
    true
}

/// Rows per mouse-wheel notch for the chat transcript.
///
/// Public for unit tests; step scales with viewport so one notch is visible
/// on both short and tall terminals.
pub fn chat_wheel_step(app: &TuiApp) -> isize {
    let h = app.chat_viewport_rows.max(1) as isize;
    (h / 3).clamp(3, 12)
}

/// Fold queued chat-wheel events into a single `scroll_rows` and drop them
/// from `events`.
///
/// A trackpad flick is dozens of `ScrollUp`/`ScrollDown` plus `Moved`.
/// Handling each through `handle_mouse` is cheap; painting after each is not.
/// The run loop drains first, then this collapse, then one paint.
///
/// Dialogs / Help keep their own wheel path — leave those events in the
/// batch when a modal is open.
pub fn coalesce_chat_wheels(app: &mut TuiApp, events: &mut Vec<Event>) {
    if app.modal_is_open() {
        return;
    }
    let step = chat_wheel_step(app);
    let mut delta = 0isize;
    events.retain(|ev| match ev {
        Event::Mouse(m) => match m.kind {
            MouseEventKind::ScrollUp => {
                delta += step;
                false
            }
            MouseEventKind::ScrollDown => {
                delta -= step;
                false
            }
            _ => true,
        },
        _ => true,
    });
    if delta != 0 {
        app.scroll_rows(delta);
    }
}

/// Keep only the last `Resize` in a drained batch (jcode / Grok: a
/// window-manager snap floods `TIOCGWINSZ`; laying out each size is wasted).
pub fn coalesce_resizes(events: &mut Vec<Event>) {
    let last = events.iter().rev().find_map(|e| match e {
        Event::Resize(w, h) => Some((*w, *h)),
        _ => None,
    });
    let Some((w, h)) = last else {
        return;
    };
    events.retain(|e| !matches!(e, Event::Resize(_, _)));
    events.push(Event::Resize(w, h));
}

/// Grab offset within the chat scrollbar thumb (top-origin track math).
fn chat_scrollbar_grab_at(app: &TuiApp, row: u16, track: ratatui::layout::Rect) -> u16 {
    use crate::ui::scrollbar::{scrollbar_metrics, thumb_top_for_offset};
    let total = app.chat_scroll_total;
    let visible = app.chat_viewport_rows.max(1) as usize;
    let height = track.height as usize;
    let Some((thumb_len, max_off, travel)) = scrollbar_metrics(total, visible, height) else {
        return 0;
    };
    // Chat `scroll_offset` is bottom-anchored; convert to top-origin view_start.
    let view_start = max_off.saturating_sub(app.scroll_offset.min(max_off));
    let top = thumb_top_for_offset(view_start, max_off, travel);
    let rel = row.saturating_sub(track.y) as usize;
    if rel >= top && rel < top + thumb_len {
        (rel - top) as u16
    } else {
        (thumb_len / 2) as u16
    }
}

/// Map a pointer y on the chat scrollbar to bottom-anchored `scroll_offset`.
///
/// Chat scroll is bottom-anchored (`0` = newest). The shared helper returns a
/// top-origin `view_start`, which we invert. Track **ends snap** so the last
/// track cell always yields offset 0 (true bottom) even when the grab point is
/// the middle of a tall thumb — without this, "scroll to bottom" stopped a few
/// rows short of the latest message.
fn apply_chat_scrollbar_offset(app: &mut TuiApp, row: u16, grab: Option<u16>) {
    use crate::ui::scrollbar::{offset_from_pointer_y, scrollbar_metrics};
    let Some(track) = app.chat_scrollbar_hit else {
        return;
    };
    let total = app.chat_scroll_total;
    let visible = app.chat_viewport_rows.max(1) as usize;
    if total == 0 || visible == 0 {
        return;
    }
    let max_off = total.saturating_sub(visible);
    if max_off == 0 {
        app.scroll_offset = 0;
        app.auto_scroll = true;
        app.mark_dirty();
        return;
    }

    let height = track.height as usize;
    // Clamp into the track; dragging past the ends pins to the end.
    let y = if row < track.y {
        track.y
    } else if row >= track.y.saturating_add(track.height) {
        track.y.saturating_add(track.height.saturating_sub(1))
    } else {
        row
    };
    let rel = (y - track.y) as usize;

    let view_start = if rel == 0 {
        // Top of track → oldest content.
        0
    } else if height > 0 && rel + 1 >= height {
        // Bottom of track → newest content (bottom-anchored offset 0).
        max_off
    } else if let Some((thumb_len, _, travel)) = scrollbar_metrics(total, visible, height) {
        let grab_u = grab.unwrap_or((thumb_len / 2) as u16) as usize;
        let thumb_top = rel.saturating_sub(grab_u).min(travel);
        // Thumb flush with track bottom → force document end.
        if travel > 0 && thumb_top + thumb_len >= height {
            max_off
        } else {
            offset_from_pointer_y(y, track, total, visible, grab).min(max_off)
        }
    } else {
        offset_from_pointer_y(y, track, total, visible, grab).min(max_off)
    };

    app.scroll_offset = max_off.saturating_sub(view_start);
    app.auto_scroll = app.scroll_offset == 0;
    app.mark_dirty();
}

/// Mouse while any modal is open (dialog stack **or** Help overlay).
///
/// Shared contract for every popup:
/// - wheel scrolls the modal body (list / help), not the chat behind
/// - drag-select copies only cells inside `dialog_modal_hit`
/// - `[✗]` dismisses
/// - scrollbar drag when present
fn handle_modal_mouse(app: &mut TuiApp, mouse: MouseEvent) -> bool {
    let active = app.dialogs.active().cloned();
    let help_only = app.mode == AppMode::Help && active.is_none();

    match mouse.kind {
        MouseEventKind::ScrollDown => {
            if help_only || matches!(active, Some(DialogKind::Help)) {
                app.help_scroll = app.help_scroll.saturating_add(3);
            } else if let Some(ref d) = active {
                move_in_dialog(app, d, 1);
            }
            app.mark_dirty();
        }
        MouseEventKind::ScrollUp => {
            if help_only || matches!(active, Some(DialogKind::Help)) {
                app.help_scroll = app.help_scroll.saturating_sub(3);
            } else if let Some(ref d) = active {
                move_in_dialog(app, d, -1);
            }
            app.mark_dirty();
        }
        MouseEventKind::Down(MouseButton::Left) => {
            // [✗] first — dismiss immediately. Do not arm text selection: a
            // 1-cell pointer jitter was marking the click as a drag and the
            // Up handler returned before ever checking the close control.
            if app.dialog_close_contains(mouse.column, mouse.row) {
                app.mouse_sel = None;
                app.dialog_scrollbar_grab = None;
                dismiss_modal(app);
                return true;
            }

            // Scrollbar: start a thumb drag or jump on track click.
            if app.dialog_scrollbar_contains(mouse.column, mouse.row)
                && let Some(track) = app.dialog_scrollbar_hit
            {
                let grab = scrollbar_grab_at(app, mouse.row, track);
                app.dialog_scrollbar_grab = Some(grab);
                apply_modal_scrollbar(app, active.as_ref(), mouse.row, Some(grab));
                app.mouse_sel = None;
                app.mark_dirty();
                return true;
            }

            // Outside the modal: no selection (background is not copyable).
            if app.dialog_modal_hit.is_some() && !app.dialog_modal_contains(mouse.column, mouse.row)
            {
                app.dialog_scrollbar_grab = None;
                app.mouse_sel = None;
                app.mark_dirty();
                return true;
            }

            // Track click origin; confirm on Up only if it was a click not a drag.
            // Clamp into the modal so a press on the border stays inside.
            let (ax, ay) = app.clamp_to_dialog_modal(mouse.column, mouse.row);
            app.dialog_scrollbar_grab = None;
            app.mouse_sel = Some(MouseSelection {
                anchor_x: ax,
                anchor_y: ay,
                focus_x: ax,
                focus_y: ay,
                dragging: false,
            });
            app.mark_dirty();
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if app.dialog_scrollbar_grab.is_some() {
                let grab = app.dialog_scrollbar_grab;
                apply_modal_scrollbar(app, active.as_ref(), mouse.row, grab);
                app.mark_dirty();
                return true;
            }
            // Clamp focus into the modal before mutably borrowing selection.
            let (fx, fy) = app.clamp_to_dialog_modal(mouse.column, mouse.row);
            if let Some(sel) = &mut app.mouse_sel {
                sel.focus_x = fx;
                sel.focus_y = fy;
                if sel.anchor_x != sel.focus_x || sel.anchor_y != sel.focus_y {
                    sel.dragging = true;
                }
                app.mark_dirty();
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            // End scrollbar drag without treating it as a list-row click.
            if app.dialog_scrollbar_grab.take().is_some() {
                app.mouse_sel = None;
                app.mark_dirty();
                return true;
            }

            let col = mouse.column;
            let row = mouse.row;

            // [✗] wins over drag-copy (release on close still dismisses).
            if app.dialog_close_contains(col, row) {
                app.mouse_sel = None;
                dismiss_modal(app);
                return true;
            }

            let was_drag = app.mouse_sel.as_ref().map(|s| s.dragging).unwrap_or(false);

            // Drag selection → copy only cells inside the modal.
            if was_drag {
                copy_modal_selection(app, col, row);
                app.mark_dirty();
                return true;
            }

            app.mouse_sel = None;

            // Click a row → select (and for pickers, confirm immediately).
            if let Some(ref active) = active
                && let Some(idx) = app.dialog_list_index_at(col, row)
            {
                match active {
                    DialogKind::SessionList => {
                        app.session_list.selected = idx;
                        confirm_dialog(app, active);
                    }
                    DialogKind::Sessions => {
                        app.sessions_cursor = idx;
                        confirm_dialog(app, active);
                    }
                    DialogKind::Model => {
                        app.model_selection.selected = idx;
                        confirm_dialog(app, active);
                    }
                    DialogKind::Agent => {
                        app.agent_picker_selected = idx;
                        confirm_dialog(app, active);
                    }
                    DialogKind::Theme => {
                        app.theme_selected = idx;
                        confirm_dialog(app, active);
                    }
                    DialogKind::Login => {
                        app.login_dialog.selected = idx;
                        confirm_dialog(app, active);
                    }
                    DialogKind::Effort => {
                        app.effort_picker_selected = idx;
                        confirm_dialog(app, active);
                    }
                    DialogKind::Provider
                        if app.provider_dialog.mode == crate::app::ProviderDialogMode::Select =>
                    {
                        app.provider_dialog.selected = idx;
                        confirm_dialog(app, active);
                    }
                    DialogKind::Question(_) => {
                        // Bottom questionnaire: click option row.
                        if let Some(DialogKind::Question(mut st)) = app.dialogs.pop() {
                            st.set_cursor(idx);
                            if st.is_other_index(idx) {
                                st.free_text_focus = true;
                                app.dialogs.push(DialogKind::Question(st));
                            } else if st.current().map(|q| q.multi_select).unwrap_or(false) {
                                st.toggle_multi_at_cursor();
                                app.dialogs.push(DialogKind::Question(st));
                            } else if let Some(answers) = st.confirm_current() {
                                // Finished — run loop sends Ok via pending_question_answers.
                                app.pending_question_answers = Some(answers);
                                app.mode = AppMode::Normal;
                                app.key_context = KeymapContext::Normal;
                                app.clear_dialog_hits();
                            } else {
                                // Advanced to next question in multi-q set.
                                app.dialogs.push(DialogKind::Question(st));
                            }
                        }
                    }
                    _ => {
                        // List dialogs without click-to-confirm: just move highlight.
                        move_in_dialog_to(app, active, idx);
                    }
                }
                app.mark_dirty();
            }
        }
        MouseEventKind::Moved => {
            // Hover over [✗] needs a repaint for the color change.
            // (Position already stored in handle_mouse; dirty always so hover
            // updates even when the host only sends Move without Press.)
            app.mark_dirty();
        }
        _ => {}
    }
    true
}

/// Copy a finished drag selection, clipped to the active modal rect.
fn copy_modal_selection(app: &mut TuiApp, col: u16, row: u16) {
    let Some(sel) = app.mouse_sel.take() else {
        return;
    };
    let (fx, fy) = app.clamp_to_dialog_modal(col, row);
    let text = if let Some(modal) = app.dialog_modal_hit {
        crate::clipboard::text_from_cells_clipped(
            &app.screen_cells,
            sel.anchor_x,
            sel.anchor_y,
            fx,
            fy,
            crate::clipboard::ClipRect::from_ratatui(modal),
        )
    } else {
        crate::clipboard::text_from_cells(&app.screen_cells, sel.anchor_x, sel.anchor_y, fx, fy)
    };
    if text.is_empty() {
        return;
    }
    if crate::clipboard::copy_text(&text) {
        app.toasts.push(
            crate::toast::ToastKind::Info,
            format!("Copied {} chars", text.chars().count()),
        );
    } else {
        app.toasts.push(
            crate::toast::ToastKind::Warning,
            "Copy failed — no clipboard",
        );
    }
}

/// Grab offset within the thumb for a press on the scrollbar track.
fn scrollbar_grab_at(app: &TuiApp, row: u16, track: ratatui::layout::Rect) -> u16 {
    use crate::ui::scrollbar::{scrollbar_metrics, thumb_top_for_offset};
    let total = app.dialog_list_total;
    let visible = app.dialog_list_visible.max(1);
    let height = track.height as usize;
    let Some((thumb_len, max_off, travel)) = scrollbar_metrics(total, visible, height) else {
        return 0;
    };
    let offset = app.dialog_list_scroll_start;
    let top = thumb_top_for_offset(offset, max_off, travel);
    let rel = row.saturating_sub(track.y) as usize;
    if rel >= top && rel < top + thumb_len {
        (rel - top) as u16
    } else {
        // Track click: pin grab to thumb middle so the jump feels centered.
        (thumb_len / 2) as u16
    }
}

/// Scroll help or a list so the viewport matches a pointer y on the scrollbar.
fn apply_modal_scrollbar(
    app: &mut TuiApp,
    active: Option<&DialogKind>,
    row: u16,
    grab: Option<u16>,
) {
    use crate::ui::scrollbar::offset_from_pointer_y;
    let Some(track) = app.dialog_scrollbar_hit else {
        return;
    };
    let total = app.dialog_list_total;
    let visible = app.dialog_list_visible.max(1);
    if total == 0 {
        return;
    }
    let offset = offset_from_pointer_y(row, track, total, visible, grab);
    let help = app.mode == AppMode::Help
        || matches!(active, Some(DialogKind::Help))
        || (active.is_none() && app.mode == AppMode::Help);
    if help {
        let max_off = total.saturating_sub(visible);
        app.help_scroll = offset.min(max_off);
        app.dialog_list_scroll_start = app.help_scroll;
        return;
    }
    if let Some(d) = active {
        use crate::ui::scrollbar::selection_for_offset;
        let sel = selection_for_offset(offset, total, visible);
        move_in_dialog_to(app, d, sel);
    }
}

/// Dismiss Help overlay or the top dialog — shared by Esc and `[✗]`.
fn dismiss_modal(app: &mut TuiApp) {
    if app.mode == AppMode::Help && !app.dialogs.is_open() {
        if app.help_searching || !app.help_query.is_empty() {
            app.help_query.clear();
            app.help_searching = false;
            app.help_scroll = 0;
            app.mark_dirty();
            return;
        }
        app.mode = AppMode::Normal;
        app.key_context = KeymapContext::Normal;
        app.mouse_sel = None;
        app.dialog_scrollbar_grab = None;
        app.clear_dialog_hits();
        app.mark_dirty();
        return;
    }
    dismiss_dialog(app);
}

/// Open the Keyboard Shortcuts cheatsheet (clears any leftover filter).
fn open_help(app: &mut TuiApp) {
    app.mode = AppMode::Help;
    app.key_context = KeymapContext::Help;
    app.help_scroll = 0;
    app.help_query.clear();
    app.help_searching = false;
}

/// Type into the cheatsheet search bar. Returns true when the key was consumed.
fn handle_help_type(app: &mut TuiApp, key: &KeyEvent) -> bool {
    match key.code {
        KeyCode::Up => {
            app.help_scroll = app.help_scroll.saturating_sub(1);
            true
        }
        KeyCode::Down => {
            app.help_scroll = app.help_scroll.saturating_add(1);
            true
        }
        KeyCode::Char('k') if !app.help_searching && app.help_query.is_empty() => {
            app.help_scroll = app.help_scroll.saturating_sub(1);
            true
        }
        KeyCode::Char('j') if !app.help_searching && app.help_query.is_empty() => {
            app.help_scroll = app.help_scroll.saturating_add(1);
            true
        }
        KeyCode::Char('q') if !app.help_searching && app.help_query.is_empty() => {
            // Grok Esc · Whycode `q` — close even when key_context is stale.
            app.mode = AppMode::Normal;
            app.key_context = KeymapContext::Normal;
            true
        }
        KeyCode::Char('/') if !app.help_searching => {
            app.help_searching = true;
            app.help_scroll = 0;
            true
        }
        KeyCode::Char(c) if app.help_searching || !app.help_query.is_empty() => {
            if !c.is_control() {
                app.help_searching = true;
                app.help_query.push(c);
                app.help_scroll = 0;
            }
            true
        }
        KeyCode::Backspace if app.help_searching || !app.help_query.is_empty() => {
            if app.help_query.pop().is_none() {
                app.help_searching = false;
            }
            app.help_scroll = 0;
            true
        }
        KeyCode::Esc if app.help_searching || !app.help_query.is_empty() => {
            app.help_query.clear();
            app.help_searching = false;
            app.help_scroll = 0;
            true
        }
        _ => false,
    }
}

/// Pop the active dialog and leave dialog mode when the stack is empty.
fn dismiss_dialog(app: &mut TuiApp) {
    // Questionnaire oneshot must complete — signal the run loop.
    if matches!(app.dialogs.active(), Some(DialogKind::Question(_))) {
        app.question_dismissed = true;
    }
    app.dialogs.pop();
    app.mouse_sel = None;
    app.dialog_scrollbar_grab = None;
    if !app.dialogs.is_open() {
        app.mode = AppMode::Normal;
        app.key_context = KeymapContext::Normal;
        app.clear_dialog_hits();
    }
    app.mark_dirty();
}

/// Jump the list cursor to an absolute index (clamped).
fn move_in_dialog_to(app: &mut TuiApp, active: &DialogKind, idx: usize) {
    use crate::app::ProviderDialogMode;
    match active {
        DialogKind::Provider if app.provider_dialog.mode == ProviderDialogMode::AddCustom => {}
        DialogKind::Provider => {
            let len = app.provider_dialog.providers.len() + 1;
            if len > 0 {
                app.provider_dialog.selected = idx.min(len - 1);
            }
        }
        DialogKind::Model => {
            let len = app.model_selection.models.len();
            if len > 0 {
                app.model_selection.selected = idx.min(len - 1);
            }
        }
        DialogKind::Agent => {
            let len = app.primary_agents.len();
            if len > 0 {
                app.agent_picker_selected = idx.min(len - 1);
            }
        }
        DialogKind::SessionList => {
            let len = app.session_list.sessions.len();
            if len > 0 {
                app.session_list.selected = idx.min(len - 1);
            }
        }
        DialogKind::Sessions => {
            let len = app.sessions_rows.len();
            if len > 0 {
                app.sessions_cursor = idx.min(len - 1);
            }
        }
        DialogKind::Theme => {
            let len = crate::theme::ThemeName::ALL.len();
            if len > 0 {
                app.theme_selected = idx.min(len - 1);
            }
        }
        DialogKind::Login => {
            let len = app.login_dialog.rows.len();
            if len > 0 {
                app.login_dialog.selected = idx.min(len - 1);
            }
        }
        DialogKind::Effort => {
            let len = effort_levels(app).len();
            if len > 0 {
                app.effort_picker_selected = idx.min(len - 1);
            }
        }
        DialogKind::Question(_) => {
            if let Some(DialogKind::Question(mut st)) = app.dialogs.pop() {
                st.set_cursor(idx);
                app.dialogs.push(DialogKind::Question(st));
            }
        }
        _ => {}
    }
}

// ── Dialog Key Handling ────────────────────────────────────────────────
fn handle_dialog_key(app: &mut TuiApp, key: &KeyEvent) -> bool {
    let action = crate::keymap::Keymap::new().resolve(KeymapContext::Dialog, app.focus, key);

    let active = match app.dialogs.active() {
        Some(d) => d.clone(),
        None => {
            // No dialog — exit dialog mode.
            app.mode = AppMode::Normal;
            app.key_context = KeymapContext::Normal;
            app.dialogs.clear();
            return true;
        }
    };

    if matches!(active, DialogKind::Help) && handle_help_type(app, key) {
        app.mark_dirty();
        return true;
    }

    match action {
        Some(Action::DialogCancel) => {
            if matches!(active, DialogKind::Help)
                && (app.help_searching || !app.help_query.is_empty())
            {
                app.help_query.clear();
                app.help_searching = false;
                app.help_scroll = 0;
                app.mark_dirty();
                return true;
            }
            dismiss_dialog(app);
        }
        Some(Action::DialogConfirm) => {
            confirm_dialog(app, &active);
        }
        // Up/Down are bound to next/prev field. In a form that means the next
        // input; in a list dialog it means the next row.
        Some(Action::DialogNextField) => {
            move_in_dialog(app, &active, 1);
            app.mark_dirty();
        }
        Some(Action::DialogPrevField) => {
            move_in_dialog(app, &active, -1);
            app.mark_dirty();
        }
        Some(Action::InputBackspace) => {
            if matches!(active, DialogKind::Provider) {
                let field_val = match app.provider_dialog.active_field {
                    0 => &mut app.provider_dialog.form_name,
                    1 => &mut app.provider_dialog.form_api_key,
                    2 => &mut app.provider_dialog.form_base_url,
                    3 => &mut app.provider_dialog.form_headers,
                    _ => return true,
                };
                field_val.pop();
            }
        }
        _ => {
            // Ctrl+W on the session picker: close the selected live session.
            if matches!(active, DialogKind::SessionList)
                && key.code == KeyCode::Char('w')
                && key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL)
            {
                if let Some(entry) = app.session_list.sessions.get(app.session_list.selected)
                    && let Some(idx) = entry.live
                {
                    app.session_list.pending_close = Some(idx);
                    dismiss_dialog(app);
                } else {
                    app.toasts.push(
                        crate::toast::ToastKind::Info,
                        "Only live sessions close here — persisted ones stay in history",
                    );
                }
                return true;
            }
            // PageUp / PageDown jump by viewport size (list pickers).
            let page = app.dialog_list_visible.max(1) as isize;
            match key.code {
                KeyCode::PageDown => {
                    move_in_dialog(app, &active, page);
                    app.mark_dirty();
                    return true;
                }
                KeyCode::PageUp => {
                    move_in_dialog(app, &active, -page);
                    app.mark_dirty();
                    return true;
                }
                KeyCode::Home => {
                    move_in_dialog_to(app, &active, 0);
                    app.mark_dirty();
                    return true;
                }
                KeyCode::End => {
                    let last = app.dialog_list_total.saturating_sub(1);
                    move_in_dialog_to(app, &active, last);
                    app.mark_dirty();
                    return true;
                }
                _ => {}
            }
            // Forward char input to provider form fields. Only in the add-custom
            // form: in select mode a keystroke is navigation, not text.
            if matches!(active, DialogKind::Provider)
                && let KeyCode::Char(c) = key.code
                && app.provider_dialog.mode == crate::app::ProviderDialogMode::AddCustom
            {
                let field_val = match app.provider_dialog.active_field {
                    0 => &mut app.provider_dialog.form_name,
                    1 => &mut app.provider_dialog.form_api_key,
                    2 => &mut app.provider_dialog.form_base_url,
                    3 => &mut app.provider_dialog.form_headers,
                    _ => return true,
                };
                field_val.push(c);
            }
            if matches!(active, DialogKind::Help) && handle_help_type(app, key) {
                app.mark_dirty();
                return true;
            }
        }
    }

    true
}

fn confirm_dialog(app: &mut TuiApp, dialog: &DialogKind) {
    match dialog {
        DialogKind::Confirm { on_confirm, .. } => match on_confirm {
            ConfirmAction::Quit => {
                app.running = false;
            }
            ConfirmAction::ClearSession => {
                app.messages.clear();
                app.status_message = "Session cleared".into();
            }
            ConfirmAction::DeleteProvider(name) => {
                app.status_message = format!("Provider '{name}' would be deleted");
            }
        },
        DialogKind::Alert { .. } => {
            // Close alert on confirm.
        }
        DialogKind::Model => {
            if let Some((p, m)) = app
                .model_selection
                .models
                .get(app.model_selection.selected)
                .cloned()
            {
                app.pending_model = Some((p, m));
            }
        }
        DialogKind::Agent => {
            if let Some(name) = app.primary_agents.get(app.agent_picker_selected).cloned() {
                app.pending_agent = Some(name);
            }
        }
        DialogKind::SessionList => {
            if let Some(entry) = app.session_list.sessions.get(app.session_list.selected) {
                match entry.live {
                    // Live parked runtime → switch; live active → no-op close.
                    Some(idx) => {
                        app.pending_session_switch =
                            Some(if idx == usize::MAX { usize::MAX } else { idx });
                    }
                    None => {
                        app.pending_session_id = Some(entry.id.clone());
                    }
                }
            }
        }
        DialogKind::Sessions => {
            if let Some(row) = app.sessions_rows.get(app.sessions_cursor) {
                app.pending_session_switch = Some(row.parked_idx.unwrap_or(usize::MAX));
            }
        }
        DialogKind::Theme => {
            use crate::theme::ThemeName;
            if let Some(t) = ThemeName::ALL.get(app.theme_selected).copied() {
                app.theme = t;
                app.config.theme = t;
                // Drop file-override so the built-in palette is visible immediately.
                app.config.theme_override = None;
                app.config.extra = crate::theme::ExtraColors::default();
                t.apply_syntax_theme();
                for msg in &mut app.messages {
                    msg.invalidate_layout();
                }
                app.status_message = format!("Theme → {}", t.name());
                app.toasts.push(
                    crate::toast::ToastKind::Success,
                    format!("Theme · {}", t.name()),
                );
            }
        }
        DialogKind::Login => {
            if let Some(row) = app.login_dialog.rows.get(app.login_dialog.selected) {
                app.pending_login_provider = Some(row.provider.clone());
            }
        }
        DialogKind::Effort => {
            if let Some(level) = effort_levels(app).get(app.effort_picker_selected) {
                app.pending_effort = Some(level.as_str().to_string());
            }
        }
        _ => {}
    }
    dismiss_dialog(app);
}

// ── Command Execution ──────────────────────────────────────────────────
fn execute_command(app: &mut TuiApp, cmd: &str) {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    match parts.first().copied() {
        Some(":q") | Some(":quit") => {
            app.running = false;
        }
        Some(":h") | Some(":help") => {
            open_help(app);
        }
        Some(":provider") | Some(":prov") => {
            open_provider_dialog(app);
        }
        Some(":model") => {
            open_model_dialog(app);
        }
        Some(":theme") => {
            app.mode = AppMode::Dialog;
            app.key_context = KeymapContext::Dialog;
            app.dialogs.push(DialogKind::Theme);
        }
        Some(":clear") => {
            app.messages.clear();
            app.status_message = "Session cleared".into();
        }
        Some(":sidebar") => {
            app.sidebar.visible = !app.sidebar.visible;
            app.mark_dirty();
        }
        _ => {
            app.status_message = format!("Unknown command: {}", cmd);
        }
    }
}

fn open_provider_dialog(app: &mut TuiApp) {
    // Populate provider list from config.
    app.provider_dialog.providers.clear();
    // Built-in providers first.
    let builtin = [
        "opencode",
        "anthropic",
        "openai",
        "google",
        "google-antigravity",
        "groq",
        "deepseek",
        "mistral",
        "azure",
    ];
    for name in builtin {
        app.provider_dialog.providers.push(name.to_string());
    }
    // Then custom from config.
    if let Ok(config) = whycode_config::Config::load() {
        for name in config.providers.keys() {
            if !app.provider_dialog.providers.contains(name) {
                app.provider_dialog.providers.push(name.clone());
            }
        }
    }
    app.provider_dialog.mode = crate::app::ProviderDialogMode::Select;
    app.provider_dialog.selected = 0;
    app.mode = AppMode::Dialog;
    app.key_context = KeymapContext::Dialog;
    app.dialogs.push(DialogKind::Provider);
}

/// Open `dialog`, putting the app into dialog mode.
pub fn open_dialog(app: &mut TuiApp, dialog: DialogKind) {
    app.mode = AppMode::Dialog;
    app.key_context = KeymapContext::Dialog;
    app.dialogs.push(dialog);
}

/// Open the primary-agent picker, highlighting the current agent.
pub fn open_agent_dialog(app: &mut TuiApp) {
    app.agent_picker_selected = app
        .primary_agents
        .iter()
        .position(|n| n == &app.agent_name)
        .unwrap_or(app.agent_cycle_idx);
    open_dialog(app, DialogKind::Agent);
}

/// Open the model picker, highlighting the active provider/model.
pub fn open_model_dialog(app: &mut TuiApp) {
    if app.model_selection.models.is_empty() {
        fill_model_catalog_from_disk(app);
    }
    app.model_selection.selected = app
        .model_selection
        .models
        .iter()
        .position(|(p, m)| p == &app.provider_name && m == &app.model_name)
        .unwrap_or(0);
    open_dialog(app, DialogKind::Model);
}

pub fn open_effort_dialog(app: &mut TuiApp) {
    let levels = effort_levels(app);
    if levels.is_empty() {
        app.toasts.push(
            crate::toast::ToastKind::Info,
            "This model has no reasoning-effort levels",
        );
        app.mark_dirty();
        return;
    }
    let current = whycode_llm::ThinkingConfig::resolve_effort(
        &app.provider_name,
        &app.model_name,
        app.reasoning_effort.as_deref(),
    );
    app.effort_picker_selected = current
        .and_then(|cur| levels.iter().position(|e| *e == cur))
        .unwrap_or(0);
    open_dialog(app, DialogKind::Effort);
}

fn effort_levels(app: &TuiApp) -> &'static [whycode_llm::ReasoningEffort] {
    whycode_llm::ThinkingConfig::supported_efforts(&app.provider_name, &app.model_name)
}

fn fill_model_catalog_from_disk(app: &mut TuiApp) {
    let Ok(config) = whycode_config::Config::load() else {
        return;
    };
    app.model_selection.models = crate::app::catalog_models(&config);
}

/// Move the cursor within whichever dialog is showing a list.
///
/// The provider dialog has two modes and only one of them is a list: in
/// add-custom it is a form, where up and down move between input fields.
fn move_in_dialog(app: &mut TuiApp, active: &DialogKind, delta: isize) {
    use crate::app::{ProviderDialogMode, move_selection};
    match active {
        DialogKind::Provider if app.provider_dialog.mode == ProviderDialogMode::AddCustom => {
            app.provider_dialog.active_field =
                move_selection(app.provider_dialog.active_field, 4, delta);
        }
        DialogKind::Provider => {
            app.provider_dialog.selected = move_selection(
                app.provider_dialog.selected,
                app.provider_dialog.providers.len(),
                delta,
            );
        }
        DialogKind::Model => {
            app.model_selection.selected = move_selection(
                app.model_selection.selected,
                app.model_selection.models.len(),
                delta,
            );
        }
        DialogKind::Agent => {
            app.agent_picker_selected =
                move_selection(app.agent_picker_selected, app.primary_agents.len(), delta);
        }
        DialogKind::SessionList => {
            app.session_list.selected = move_selection(
                app.session_list.selected,
                app.session_list.sessions.len(),
                delta,
            );
        }
        DialogKind::Sessions => {
            app.sessions_cursor =
                move_selection(app.sessions_cursor, app.sessions_rows.len(), delta);
        }
        DialogKind::Theme => {
            app.theme_selected = move_selection(
                app.theme_selected,
                crate::theme::ThemeName::ALL.len(),
                delta,
            );
        }
        DialogKind::Login => {
            app.login_dialog.selected = move_selection(
                app.login_dialog.selected,
                app.login_dialog.rows.len(),
                delta,
            );
        }
        DialogKind::Effort => {
            app.effort_picker_selected =
                move_selection(app.effort_picker_selected, effort_levels(app).len(), delta);
        }
        DialogKind::Question(_) => {
            if let Some(DialogKind::Question(mut st)) = app.dialogs.pop() {
                if !st.free_text_focus {
                    st.move_cursor(delta);
                }
                app.dialogs.push(DialogKind::Question(st));
            }
        }
        _ => {}
    }
}

// ── Dialog helpers ─────────────────────────────────────────────────────

#[cfg(test)]
mod utf8_cursor_tests {
    use super::*;

    #[test]
    fn insert_multibyte_advances_by_utf8_len() {
        let mut buf = String::new();
        let mut cursor = 0usize;
        for c in ['ş', 'a', 'ğ'] {
            let pos = clamp_cursor(&buf, cursor);
            buf.insert(pos, c);
            cursor = pos + c.len_utf8();
        }
        assert_eq!(buf, "şağ");
        assert_eq!(cursor, buf.len());
        assert!(buf.is_char_boundary(cursor));
    }

    #[test]
    fn backspace_deletes_whole_grapheme_bytes() {
        let mut buf = String::from("şa");
        let mut cursor = buf.len();
        let end = clamp_cursor(&buf, cursor);
        let start = prev_boundary(&buf, end);
        buf.replace_range(start..end, "");
        cursor = start;
        assert_eq!(buf, "ş");
        assert_eq!(cursor, "ş".len());

        let end = clamp_cursor(&buf, cursor);
        let start = prev_boundary(&buf, end);
        buf.replace_range(start..end, "");
        cursor = start;
        assert_eq!(buf, "");
        assert_eq!(cursor, 0);
    }

    #[test]
    fn left_right_stay_on_char_boundaries() {
        let s = "şxğ";
        let mut c = s.len();
        c = prev_boundary(s, c);
        assert!(s.is_char_boundary(c));
        assert_eq!(&s[..c], "şx");
        c = prev_boundary(s, c);
        assert_eq!(&s[..c], "ş");
        c = next_boundary(s, c);
        assert_eq!(&s[..c], "şx");
    }

    #[test]
    fn clamp_and_boundaries_cover_edges() {
        let s = "şa";
        assert_eq!(clamp_cursor(s, 0), 0);
        assert_eq!(clamp_cursor(s, s.len()), s.len());
        assert_eq!(clamp_cursor(s, 1), 0, "mid-codepoint snaps back");
        assert_eq!(clamp_cursor(s, 99), s.len());
        assert_eq!(prev_boundary("", 0), 0);
        assert_eq!(prev_boundary(s, 0), 0);
        assert_eq!(next_boundary(s, s.len()), s.len());
        assert_eq!(next_boundary("", 0), 0);
        assert!(is_bare_slash_draft("/"));
        assert!(is_bare_slash_draft("//"));
        assert!(!is_bare_slash_draft(""));
        assert!(!is_bare_slash_draft("/h"));
    }
}

#[cfg(test)]
mod event_tests {
    use super::*;
    use crate::app::{
        AgentState, ChatBlock, ChatRole, ConfirmAction, DialogKind, FocusPane, SidebarTab, TuiApp,
    };
    use crate::config::TuiAppConfig;
    use crate::keymap::KeymapContext;
    use crate::theme::ThemeName;
    use crossterm::event::{KeyModifiers, MouseButton};
    use ratatui::layout::Rect;

    fn app() -> TuiApp {
        TuiApp::new(TuiAppConfig::default())
    }

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ctrl(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    #[test]
    fn handle_event_ignores_key_release_and_keeps_running_on_resize() {
        let mut a = app();
        let mut rel = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        rel.kind = KeyEventKind::Release;
        assert!(handle_event(&mut a, Event::Key(rel)));
        assert!(a.input_buffer.is_empty());
        assert!(handle_event(&mut a, Event::Resize(120, 40)));
        assert!(
            a.needs_redraw,
            "resize (OSK / rotate) must dirty so the next paint uses the new size"
        );
        assert!(handle_event(&mut a, Event::FocusGained));
    }

    #[test]
    fn typing_and_editing_keys_update_the_prompt() {
        let mut a = app();
        assert!(handle_event(&mut a, key(KeyCode::Char('h'))));
        assert!(handle_event(&mut a, key(KeyCode::Char('i'))));
        assert_eq!(a.input_buffer, "hi");
        assert_eq!(a.input_cursor, 2);

        assert!(handle_event(&mut a, key(KeyCode::Left)));
        assert_eq!(a.input_cursor, 1);
        assert!(handle_event(&mut a, key(KeyCode::Right)));
        assert_eq!(a.input_cursor, 2);
        assert!(handle_event(&mut a, key(KeyCode::Home)));
        assert_eq!(a.input_cursor, 0);
        assert!(handle_event(&mut a, key(KeyCode::End)));
        assert_eq!(a.input_cursor, 2);

        assert!(handle_event(&mut a, key(KeyCode::Backspace)));
        assert_eq!(a.input_buffer, "h");
        a.input_cursor = 0;
        assert!(handle_event(&mut a, key(KeyCode::Delete)));
        assert!(a.input_buffer.is_empty());

        a.input_buffer = "ab".into();
        a.input_cursor = 1;
        assert!(handle_event(
            &mut a,
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT))
        ));
        assert_eq!(a.input_buffer, "a\nb");

        a.input_buffer = "keep".into();
        a.input_cursor = 4;
        assert!(handle_event(&mut a, ctrl('u')));
        assert!(a.input_buffer.is_empty());
        assert_eq!(a.input_cursor, 0);
    }

    #[test]
    fn history_up_down_walk_the_prompt_stack() {
        let mut a = app();
        a.input_history = vec!["one".into(), "two".into()];
        a.input_history_idx = 2;
        assert!(handle_event(&mut a, key(KeyCode::Up)));
        assert_eq!(a.input_buffer, "two");
        assert!(handle_event(&mut a, key(KeyCode::Up)));
        assert_eq!(a.input_buffer, "one");
        assert!(handle_event(&mut a, key(KeyCode::Down)));
        assert_eq!(a.input_buffer, "two");
        assert!(handle_event(&mut a, key(KeyCode::Down)));
        assert!(a.input_buffer.is_empty());
    }

    #[test]
    fn quit_help_command_and_escape_modes() {
        let mut a = app();
        assert!(handle_event(&mut a, ctrl('c')));
        assert_eq!(a.mode, AppMode::Dialog);
        assert!(matches!(
            a.dialogs.active(),
            Some(DialogKind::Confirm {
                on_confirm: ConfirmAction::Quit,
                ..
            })
        ));
        assert!(handle_event(&mut a, key(KeyCode::Esc)));
        assert_eq!(a.mode, AppMode::Normal);

        a.mode = AppMode::Help;
        a.key_context = KeymapContext::Help;
        assert!(handle_event(&mut a, key(KeyCode::Char('?'))));
        assert_eq!(a.mode, AppMode::Help);
        assert!(handle_event(&mut a, key(KeyCode::Char('q'))));
        assert_eq!(a.mode, AppMode::Normal);

        a.input_buffer.clear();
        a.input_cursor = 0;
        assert!(handle_event(&mut a, key(KeyCode::Char('?'))));
        assert_eq!(a.mode, AppMode::Normal);
        assert_eq!(a.input_buffer, "?");

        assert!(handle_event(&mut a, key(KeyCode::Char(':'))));
        assert_eq!(a.mode, AppMode::Command);
        assert_eq!(a.command.buffer, ":");
        assert!(handle_event(&mut a, key(KeyCode::Esc)));
        assert_eq!(a.mode, AppMode::Normal);
        assert!(a.command.buffer.is_empty());
    }

    #[test]
    fn escape_steals_slash_file_busy_and_double_clears() {
        let mut a = app();
        a.input_buffer = "/".into();
        a.input_cursor = 1;
        a.slash_suggest.refresh(&a.input_buffer);
        assert!(a.slash_suggest.active);
        handle_event(&mut a, key(KeyCode::Esc));
        assert!(!a.slash_suggest.active);
        assert!(a.input_buffer.is_empty(), "bare slash draft is dropped");

        a.file_suggest.active = true;
        a.input_buffer = "@src".into();
        handle_event(&mut a, key(KeyCode::Esc));
        assert!(!a.file_suggest.active);
        assert_eq!(a.input_buffer, "@src");

        a.current_agent_state = AgentState::Generating;
        a.input_buffer = "keep".into();
        handle_event(&mut a, key(KeyCode::Esc));
        assert_eq!(a.input_buffer, "keep");
        a.current_agent_state = AgentState::Idle;

        handle_event(&mut a, key(KeyCode::Esc));
        assert!(a.esc_armed_at.is_some());
        handle_event(&mut a, key(KeyCode::Esc));
        assert!(a.input_buffer.is_empty());
        assert!(
            a.toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("cleared"))
        );

        a.focus = FocusPane::Scrollback;
        handle_event(&mut a, key(KeyCode::Esc));
        assert_eq!(a.focus, FocusPane::Prompt);
    }

    #[test]
    fn slash_reopens_on_second_slash_instead_of_doubling() {
        let mut a = app();
        a.input_buffer = "/".into();
        a.input_cursor = 1;
        handle_event(&mut a, key(KeyCode::Char('/')));
        assert_eq!(a.input_buffer, "/");
        assert!(a.slash_suggest.active);
    }

    #[test]
    fn file_suggest_keys_accept_step_and_open() {
        let mut a = app();
        a.file_suggest.active = true;
        a.file_suggest.token_start = 0;
        a.file_suggest.matches = vec![
            whycode_index::FileMatch {
                rel: "a.rs".into(),
                ..Default::default()
            },
            whycode_index::FileMatch {
                rel: "b.rs".into(),
                ..Default::default()
            },
        ];
        a.input_buffer = "@x".into();
        a.input_cursor = 2;
        handle_event(&mut a, key(KeyCode::Down));
        assert_eq!(a.file_suggest.selected, 1);
        handle_event(&mut a, key(KeyCode::Up));
        assert_eq!(a.file_suggest.selected, 0);
        handle_event(&mut a, key(KeyCode::Tab));
        assert_eq!(a.input_buffer, "@a.rs ");
        assert!(!a.file_suggest.active);

        a.file_suggest.active = true;
        a.file_suggest.matches.clear();
        handle_event(&mut a, key(KeyCode::Enter));
        assert!(!a.file_suggest.active);

        handle_event(
            &mut a,
            Event::Key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL)),
        );
        assert!(a.file_suggest.active);
        assert!(a.input_buffer.contains('@'));
    }

    #[test]
    fn submit_slash_tab_and_focus_actions() {
        let mut a = app();
        a.input_buffer = "hello".into();
        handle_event(&mut a, key(KeyCode::Enter));
        assert_eq!(a.pending_prompt.as_deref(), Some("hello"));
        assert!(a.input_buffer.is_empty());

        a.input_buffer = "/".into();
        a.slash_suggest.refresh(&a.input_buffer);
        let before = a.slash_suggest.selected;
        handle_event(&mut a, key(KeyCode::Down));
        assert_ne!(a.slash_suggest.selected, before);
        handle_event(&mut a, key(KeyCode::Tab));
        assert!(a.input_buffer.starts_with('/'));

        a.pending_suggestion = Some("try this".into());
        a.input_buffer.clear();
        a.slash_suggest.dismiss();
        handle_event(&mut a, key(KeyCode::Tab));
        assert_eq!(a.input_buffer, "try this");

        a.slash_suggest.dismiss();
        handle_event(&mut a, key(KeyCode::Tab));
        assert_eq!(a.focus, FocusPane::Scrollback);
        handle_event(&mut a, key(KeyCode::Tab));
        assert_eq!(a.focus, FocusPane::Prompt);
    }

    #[test]
    fn sidebar_scroll_and_dialog_chords() {
        let mut a = app();
        assert!(!a.sidebar.visible);
        handle_event(&mut a, ctrl('b'));
        assert!(a.sidebar.visible);
        handle_event(&mut a, ctrl('g'));
        assert_eq!(a.sidebar.active_tab, SidebarTab::Agents);

        a.focus = FocusPane::Scrollback;
        handle_event(&mut a, key(KeyCode::Char(']')));
        handle_event(&mut a, key(KeyCode::Char('[')));

        handle_event(&mut a, ctrl('a'));
        assert!(!a.auto_scroll);
        handle_event(&mut a, ctrl('a'));
        assert!(a.auto_scroll);

        handle_event(&mut a, ctrl('l'));
        assert!(matches!(
            a.dialogs.active(),
            Some(DialogKind::Confirm {
                on_confirm: ConfirmAction::ClearSession,
                ..
            })
        ));
        handle_event(&mut a, key(KeyCode::Enter));
        assert!(a.messages.is_empty());

        let mut a = app();
        handle_event(&mut a, ctrl('t'));
        assert!(a.status_message.contains("switch agent"));

        handle_event(&mut a, key(KeyCode::PageUp));
        handle_event(&mut a, key(KeyCode::PageDown));
        a.focus = FocusPane::Scrollback;
        handle_event(&mut a, key(KeyCode::Home));
        handle_event(&mut a, key(KeyCode::End));

        handle_event(&mut a, ctrl('p'));
        assert!(matches!(a.dialogs.active(), Some(DialogKind::Provider)));
        handle_event(&mut a, key(KeyCode::Esc));
        handle_event(&mut a, ctrl('m'));
        assert!(matches!(a.dialogs.active(), Some(DialogKind::Model)));
    }

    #[test]
    fn command_mode_executes_colon_commands() {
        type Check = fn(&TuiApp);
        let cases: &[(&str, Check)] = &[
            (":q", |a| assert!(!a.running)),
            (":help", |a| assert_eq!(a.mode, AppMode::Help)),
            (":theme", |a| {
                assert!(matches!(a.dialogs.active(), Some(DialogKind::Theme)))
            }),
            (":model", |a| {
                assert!(matches!(a.dialogs.active(), Some(DialogKind::Model)))
            }),
            (":clear", |a| assert!(a.messages.is_empty())),
            (":sidebar", |a| assert!(a.sidebar.visible)),
            (":nope", |a| assert!(a.status_message.contains("Unknown"))),
        ];
        for (cmd, check) in cases {
            let mut a = app();
            a.add_message(ChatRole::User, "stay");
            a.mode = AppMode::Command;
            a.key_context = KeymapContext::Command;
            a.command.buffer = (*cmd).into();
            handle_event(&mut a, key(KeyCode::Enter));
            check(&a);
        }
        let mut a = app();
        a.mode = AppMode::Command;
        a.key_context = KeymapContext::Command;
        a.command.buffer = ":".into();
        handle_event(&mut a, key(KeyCode::Char('q')));
        assert_eq!(a.command.buffer, ":q");

        let mut a = app();
        a.mode = AppMode::Help;
        handle_event(&mut a, key(KeyCode::Char('q')));
        assert_eq!(a.mode, AppMode::Normal);
    }

    #[test]
    fn dialog_keys_confirm_cancel_and_list_jumps() {
        let mut a = app();
        a.confirm("Quit", "sure?", ConfirmAction::Quit);
        handle_event(&mut a, key(KeyCode::Char('y')));
        assert!(!a.running);

        let mut a = app();
        a.confirm("Clear", "?", ConfirmAction::ClearSession);
        a.add_message(ChatRole::User, "x");
        handle_event(&mut a, key(KeyCode::Enter));
        assert!(a.messages.is_empty());

        let mut a = app();
        a.alert("Hi", "there");
        handle_event(&mut a, key(KeyCode::Enter));
        assert_eq!(a.mode, AppMode::Normal);

        let mut a = app();
        a.theme_selected = 0;
        open_dialog(&mut a, DialogKind::Theme);
        handle_event(&mut a, key(KeyCode::Down));
        assert_eq!(a.theme_selected, 1);
        handle_event(&mut a, key(KeyCode::Home));
        assert_eq!(a.theme_selected, 0);
        a.dialog_list_total = ThemeName::ALL.len();
        handle_event(&mut a, key(KeyCode::End));
        assert_eq!(a.theme_selected, ThemeName::ALL.len() - 1);
        handle_event(&mut a, key(KeyCode::PageDown));
        handle_event(&mut a, key(KeyCode::PageUp));
        handle_event(&mut a, key(KeyCode::Enter));
        assert_eq!(a.mode, AppMode::Normal);

        let mut a = app();
        a.model_selection.models = vec![("acme".into(), "m1".into())];
        open_dialog(&mut a, DialogKind::Model);
        handle_event(&mut a, key(KeyCode::Enter));
        assert_eq!(
            a.pending_model
                .as_ref()
                .map(|(p, m)| (p.as_str(), m.as_str())),
            Some(("acme", "m1"))
        );

        let mut a = app();
        a.primary_agents = vec!["build".into(), "plan".into()];
        a.agent_name = "build".into();
        open_agent_dialog(&mut a);
        assert!(matches!(a.dialogs.active(), Some(DialogKind::Agent)));
        assert_eq!(a.agent_picker_selected, 0);
        handle_event(&mut a, key(KeyCode::Down));
        assert_eq!(a.agent_picker_selected, 1);
        handle_event(&mut a, key(KeyCode::Enter));
        assert_eq!(a.pending_agent.as_deref(), Some("plan"));
        assert_eq!(a.mode, AppMode::Normal);

        let mut a = app();
        a.session_list.sessions = vec![crate::app::SessionEntry {
            id: "abc".into(),
            title: "t".into(),
            messages: 1,
            updated_at: None,
            live: None,
        }];
        open_dialog(&mut a, DialogKind::SessionList);
        handle_event(&mut a, key(KeyCode::Enter));
        assert_eq!(a.pending_session_id.as_deref(), Some("abc"));

        let mut a = app();
        a.session_list.sessions = vec![crate::app::SessionEntry {
            id: "live".into(),
            title: "t".into(),
            messages: 1,
            updated_at: None,
            live: Some(2),
        }];
        open_dialog(&mut a, DialogKind::SessionList);
        handle_event(
            &mut a,
            Event::Key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL)),
        );
        assert_eq!(a.session_list.pending_close, Some(2));
    }

    #[test]
    fn provider_form_types_and_backspaces() {
        let mut a = app();
        open_provider_dialog(&mut a);
        a.provider_dialog.mode = crate::app::ProviderDialogMode::AddCustom;
        a.provider_dialog.active_field = 0;
        handle_event(&mut a, key(KeyCode::Char('x')));
        assert_eq!(a.provider_dialog.form_name, "x");
        handle_event(&mut a, key(KeyCode::Backspace));
        assert!(a.provider_dialog.form_name.is_empty());
        handle_event(&mut a, key(KeyCode::Down));
        assert_eq!(a.provider_dialog.active_field, 1);
    }

    #[test]
    fn paste_ignored_outside_normal_and_auth_esc_cancels() {
        let mut a = app();
        a.mode = AppMode::Help;
        handle_event(&mut a, Event::Paste("secret".into()));
        assert!(a.input_buffer.is_empty());

        let mut a = app();
        let (tx, _rx) = tokio::sync::oneshot::channel::<String>();
        a.auth_code_sink = Some(tx);
        handle_event(&mut a, key(KeyCode::Esc));
        assert!(a.auth_code_sink.is_none());
        assert!(a.status_message.contains("cancelled"));
    }

    #[test]
    fn mouse_click_prompt_meta_opens_agent_and_model_pickers() {
        let mut a = app();
        a.agent_hit.set_rect(Some(Rect {
            x: 40,
            y: 20,
            width: 5,
            height: 1,
        }));
        a.primary_agents = vec!["build".into(), "plan".into()];
        a.agent_name = "build".into();
        handle_event(
            &mut a,
            mouse(MouseEventKind::Down(MouseButton::Left), 42, 20),
        );
        assert!(matches!(a.dialogs.active(), Some(DialogKind::Agent)));
        assert_eq!(a.agent_picker_selected, 0);

        let mut a = app();
        a.model_hit.set_rect(Some(Rect {
            x: 50,
            y: 20,
            width: 12,
            height: 1,
        }));
        a.model_selection.models = vec![("acme".into(), "m1".into())];
        a.provider_name = "acme".into();
        a.model_name = "m1".into();
        handle_event(
            &mut a,
            mouse(MouseEventKind::Down(MouseButton::Left), 52, 20),
        );
        assert!(matches!(a.dialogs.active(), Some(DialogKind::Model)));
        assert_eq!(a.model_selection.selected, 0);

        let mut a = app();
        a.provider_name = "xai".into();
        a.model_name = "grok-4".into();
        a.effort_hit.set_rect(Some(Rect {
            x: 64,
            y: 20,
            width: 3,
            height: 1,
        }));
        handle_event(
            &mut a,
            mouse(MouseEventKind::Down(MouseButton::Left), 65, 20),
        );
        assert!(matches!(a.dialogs.active(), Some(DialogKind::Effort)));
    }

    #[test]
    fn mouse_clicks_file_and_slash_rows() {
        let mut a = app();
        a.file_suggest.active = true;
        a.file_suggest.token_start = 0;
        a.file_suggest.matches = vec![whycode_index::FileMatch {
            rel: "lib.rs".into(),
            ..Default::default()
        }];
        a.file_suggest.list_hit = Some(Rect {
            x: 0,
            y: 10,
            width: 20,
            height: 2,
        });
        a.input_buffer = "@x".into();
        handle_event(
            &mut a,
            mouse(MouseEventKind::Down(MouseButton::Left), 2, 10),
        );
        assert_eq!(a.input_buffer, "@lib.rs ");

        let mut a = app();
        a.input_buffer = "/".into();
        a.slash_suggest.refresh(&a.input_buffer);
        a.slash_suggest.list_hit = Some(Rect {
            x: 0,
            y: 8,
            width: 20,
            height: 4,
        });
        a.slash_suggest.list_scroll_start = 0;
        handle_event(&mut a, mouse(MouseEventKind::Down(MouseButton::Left), 1, 8));
        assert!(
            a.input_buffer.starts_with('/') && a.input_buffer.ends_with(' '),
            "{}",
            a.input_buffer
        );
        assert!(!a.slash_suggest.active);
    }

    #[test]
    fn mouse_subagent_and_selection_drag() {
        let mut a = app();
        a.upsert_subagent(crate::app::SubagentUpdate {
            id: "child-1".into(),
            kind: "explore".into(),
            description: "look".into(),
            status: "running".into(),
            activity: "Thinking".into(),
            elapsed_ms: 10,
            output: String::new(),
        });
        a.subagent_strip_hit.push((
            Rect {
                x: 2,
                y: 0,
                width: 10,
                height: 1,
            },
            "child-1".into(),
        ));
        handle_event(&mut a, mouse(MouseEventKind::Down(MouseButton::Left), 4, 0));
        assert_eq!(a.open_subagent.as_deref(), Some("child-1"));
        handle_event(&mut a, key(KeyCode::Char('q')));
        assert!(a.open_subagent.is_none());

        let mut a = app();
        handle_event(&mut a, mouse(MouseEventKind::Down(MouseButton::Left), 3, 5));
        assert!(a.mouse_sel.is_some());
        handle_event(&mut a, mouse(MouseEventKind::Drag(MouseButton::Left), 8, 6));
        assert_eq!(a.mouse_sel.as_ref().map(|s| s.focus_x), Some(8));
        handle_event(&mut a, mouse(MouseEventKind::Up(MouseButton::Left), 8, 6));
    }

    #[test]
    fn modal_mouse_scrolls_and_closes() {
        let mut a = app();
        open_dialog(&mut a, DialogKind::Theme);
        a.dialog_modal_hit = Some(Rect {
            x: 10,
            y: 5,
            width: 40,
            height: 12,
        });
        a.dialog_close_hit = Some(Rect {
            x: 48,
            y: 5,
            width: 3,
            height: 1,
        });
        handle_event(&mut a, mouse(MouseEventKind::ScrollDown, 20, 8));
        assert!(a.theme_selected > 0);
        handle_event(&mut a, mouse(MouseEventKind::ScrollUp, 20, 8));
        handle_event(
            &mut a,
            mouse(MouseEventKind::Down(MouseButton::Left), 49, 5),
        );
        assert!(!a.dialogs.is_open());
    }

    #[test]
    fn help_overlay_search_then_esc_clears_then_closes() {
        let mut a = app();
        a.mode = AppMode::Help;
        a.key_context = KeymapContext::Help;
        handle_event(&mut a, key(KeyCode::Char('/')));
        assert!(a.help_searching);
        handle_event(&mut a, key(KeyCode::Char('t')));
        handle_event(&mut a, key(KeyCode::Char('a')));
        handle_event(&mut a, key(KeyCode::Char('b')));
        assert_eq!(a.help_query, "tab");
        handle_event(&mut a, key(KeyCode::Esc));
        assert!(a.help_query.is_empty());
        assert!(!a.help_searching);
        assert_eq!(a.mode, AppMode::Help);
        handle_event(&mut a, key(KeyCode::Esc));
        assert_eq!(a.mode, AppMode::Normal);
    }

    #[test]
    fn help_overlay_wheel_and_close() {
        let mut a = app();
        a.mode = AppMode::Help;
        a.key_context = KeymapContext::Help;
        handle_event(&mut a, mouse(MouseEventKind::ScrollDown, 10, 10));
        assert_eq!(a.help_scroll, 3);
        handle_event(&mut a, mouse(MouseEventKind::ScrollUp, 10, 10));
        assert_eq!(a.help_scroll, 0);
        handle_event(&mut a, key(KeyCode::Esc));
        assert_eq!(a.mode, AppMode::Normal);
    }

    #[test]
    fn chat_wheel_step_and_coalesce() {
        let mut a = app();
        a.chat_viewport_rows = 30;
        assert_eq!(chat_wheel_step(&a), 10);
        a.chat_viewport_rows = 3;
        assert_eq!(chat_wheel_step(&a), 3);

        a.chat_viewport_rows = 12;
        a.chat_content_width = 40;
        for i in 0..20 {
            a.add_message(ChatRole::User, format!("line {i}"));
        }
        let mut events = vec![
            mouse(MouseEventKind::ScrollUp, 1, 1),
            mouse(MouseEventKind::ScrollUp, 1, 1),
            mouse(MouseEventKind::Moved, 1, 1),
            mouse(MouseEventKind::ScrollDown, 1, 1),
        ];
        coalesce_chat_wheels(&mut a, &mut events);
        assert_eq!(events.len(), 1, "wheels folded, move kept");
        assert!(a.scroll_offset > 0);

        a.mode = AppMode::Help;
        let n = events.len();
        coalesce_chat_wheels(&mut a, &mut events);
        assert_eq!(events.len(), n, "modal leaves the batch alone");
    }

    #[test]
    fn scrollback_select_and_open_subagent() {
        let mut a = app();
        a.add_message(ChatRole::User, "one");
        a.add_message(ChatRole::Assistant, "two");
        a.focus = FocusPane::Scrollback;
        a.selected_msg = Some(0);
        handle_event(&mut a, key(KeyCode::Down));
        assert_eq!(a.selected_msg, Some(1));
        handle_event(&mut a, key(KeyCode::Char('k')));
        assert_eq!(a.selected_msg, Some(0));
        handle_event(
            &mut a,
            Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT)),
        );
        handle_event(
            &mut a,
            Event::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT)),
        );
        handle_event(&mut a, key(KeyCode::Char('y')));
        handle_event(&mut a, key(KeyCode::Char('e')));
        handle_event(&mut a, key(KeyCode::Char('l')));

        a.upsert_subagent(crate::app::SubagentUpdate {
            id: "kid".into(),
            kind: "general".into(),
            description: "d".into(),
            status: "running".into(),
            activity: String::new(),
            elapsed_ms: 0,
            output: String::new(),
        });
        // The new system message with the Subagent block is last.
        a.selected_msg = Some(a.messages.len() - 1);
        assert!(matches!(
            a.messages.last().unwrap().blocks.first(),
            Some(ChatBlock::Subagent { .. })
        ));
        handle_event(&mut a, key(KeyCode::Enter));
        assert_eq!(a.open_subagent.as_deref(), Some("kid"));
        handle_event(&mut a, key(KeyCode::Esc));
        assert!(a.open_subagent.is_none());
    }

    #[test]
    fn execute_provider_and_unknown_colon() {
        let mut a = app();
        a.mode = AppMode::Command;
        a.key_context = KeymapContext::Command;
        a.command.buffer = ":provider".into();
        handle_event(&mut a, key(KeyCode::Enter));
        assert!(matches!(a.dialogs.active(), Some(DialogKind::Provider)));
    }

    #[test]
    fn dialog_mode_with_empty_stack_resets() {
        let mut a = app();
        a.mode = AppMode::Dialog;
        a.key_context = KeymapContext::Dialog;
        handle_event(&mut a, key(KeyCode::Esc));
        assert_eq!(a.mode, AppMode::Normal);
    }

    #[test]
    fn login_and_sessions_dashboard_confirm() {
        let mut a = app();
        a.login_dialog.rows = vec![crate::app::LoginProviderRow {
            provider: "anthropic".into(),
            label: "Anthropic".into(),
            connected: false,
        }];
        open_dialog(&mut a, DialogKind::Login);
        handle_event(&mut a, key(KeyCode::Enter));
        assert_eq!(a.pending_login_provider.as_deref(), Some("anthropic"));

        let mut a = app();
        a.sessions_rows = vec![crate::app::SessionDashboardRow {
            parked_idx: Some(3),
            title: "s".into(),
            glyph: "·".into(),
            state_label: "idle".into(),
            preview: String::new(),
            unread: false,
        }];
        open_dialog(&mut a, DialogKind::Sessions);
        handle_event(&mut a, key(KeyCode::Enter));
        assert_eq!(a.pending_session_switch, Some(3));
    }

    #[test]
    fn confirm_delete_provider_and_live_session_switch() {
        let mut a = app();
        a.confirm(
            "Delete",
            "gone?",
            ConfirmAction::DeleteProvider("acme".into()),
        );
        handle_event(&mut a, key(KeyCode::Enter));
        assert!(a.status_message.contains("acme"));
        assert_eq!(a.mode, AppMode::Normal);

        let mut a = app();
        a.session_list.sessions = vec![crate::app::SessionEntry {
            id: "live".into(),
            title: "t".into(),
            messages: 1,
            updated_at: None,
            live: Some(usize::MAX),
        }];
        open_dialog(&mut a, DialogKind::SessionList);
        handle_event(&mut a, key(KeyCode::Enter));
        assert_eq!(a.pending_session_switch, Some(usize::MAX));

        let mut a = app();
        a.session_list.sessions = vec![crate::app::SessionEntry {
            id: "persisted".into(),
            title: "t".into(),
            messages: 1,
            updated_at: None,
            live: None,
        }];
        open_dialog(&mut a, DialogKind::SessionList);
        handle_event(
            &mut a,
            Event::Key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL)),
        );
        assert!(
            a.toasts
                .visible()
                .iter()
                .any(|t| t.message.contains("persisted") || t.message.contains("live"))
        );
    }

    #[test]
    fn permission_and_question_dismiss() {
        let mut a = app();
        a.ask_permission("bash", "rm -rf /");
        assert!(matches!(
            a.dialogs.active(),
            Some(DialogKind::Permission { .. })
        ));
        handle_event(&mut a, key(KeyCode::Esc));
        assert!(!a.dialogs.is_open());

        let mut a = app();
        a.ask_question(vec![whycode_tools::question::QuestionSpec {
            prompt: "Go?".into(),
            options: vec![whycode_tools::question::QuestionOption {
                label: "Yes".into(),
                description: String::new(),
                preview: None,
            }],
            multi_select: false,
        }]);
        handle_event(&mut a, key(KeyCode::Esc));
        assert!(a.question_dismissed);
        assert!(!a.dialogs.is_open());
    }

    #[test]
    fn mouse_confirms_list_and_question_rows() {
        let mut a = app();
        a.theme_selected = 0;
        open_dialog(&mut a, DialogKind::Theme);
        a.dialog_modal_hit = Some(Rect {
            x: 10,
            y: 5,
            width: 40,
            height: 12,
        });
        a.dialog_list_hit = Some(Rect {
            x: 12,
            y: 8,
            width: 30,
            height: 6,
        });
        a.dialog_list_total = crate::theme::ThemeName::ALL.len();
        a.dialog_list_visible = 6;
        a.dialog_list_scroll_start = 0;
        handle_event(
            &mut a,
            mouse(MouseEventKind::Down(MouseButton::Left), 14, 10),
        );
        handle_event(&mut a, mouse(MouseEventKind::Up(MouseButton::Left), 14, 10));
        assert_eq!(a.mode, AppMode::Normal);

        let mut a = app();
        a.ask_question(vec![whycode_tools::question::QuestionSpec {
            prompt: "Go?".into(),
            options: vec![
                whycode_tools::question::QuestionOption {
                    label: "Yes".into(),
                    description: String::new(),
                    preview: None,
                },
                whycode_tools::question::QuestionOption {
                    label: "No".into(),
                    description: String::new(),
                    preview: None,
                },
            ],
            multi_select: false,
        }]);
        a.dialog_modal_hit = Some(Rect {
            x: 10,
            y: 5,
            width: 40,
            height: 12,
        });
        a.dialog_list_hit = Some(Rect {
            x: 12,
            y: 8,
            width: 30,
            height: 4,
        });
        a.dialog_list_total = 3;
        a.dialog_list_visible = 4;
        handle_event(
            &mut a,
            mouse(MouseEventKind::Down(MouseButton::Left), 14, 8),
        );
        handle_event(&mut a, mouse(MouseEventKind::Up(MouseButton::Left), 14, 8));
        assert!(
            a.pending_question_answers.is_some() || a.dialogs.is_open(),
            "click either finishes or keeps the questionnaire"
        );
    }

    #[test]
    fn modal_scrollbar_and_copy_selection() {
        let mut a = app();
        open_dialog(&mut a, DialogKind::Theme);
        a.dialog_modal_hit = Some(Rect {
            x: 10,
            y: 5,
            width: 40,
            height: 12,
        });
        a.dialog_scrollbar_hit = Some(Rect {
            x: 49,
            y: 6,
            width: 1,
            height: 10,
        });
        a.dialog_list_total = crate::theme::ThemeName::ALL.len();
        a.dialog_list_visible = 6;
        handle_event(
            &mut a,
            mouse(MouseEventKind::Down(MouseButton::Left), 49, 8),
        );
        assert!(a.dialog_scrollbar_grab.is_some());
        handle_event(
            &mut a,
            mouse(MouseEventKind::Drag(MouseButton::Left), 49, 12),
        );
        handle_event(&mut a, mouse(MouseEventKind::Up(MouseButton::Left), 49, 12));
        assert!(a.dialog_scrollbar_grab.is_none());

        let mut a = app();
        a.mode = AppMode::Help;
        a.key_context = KeymapContext::Help;
        a.dialog_close_hit = Some(Rect {
            x: 70,
            y: 1,
            width: 3,
            height: 1,
        });
        handle_event(
            &mut a,
            mouse(MouseEventKind::Down(MouseButton::Left), 71, 1),
        );
        assert_eq!(a.mode, AppMode::Normal);

        let mut a = app();
        open_dialog(&mut a, DialogKind::Theme);
        a.dialog_modal_hit = Some(Rect {
            x: 10,
            y: 5,
            width: 20,
            height: 8,
        });
        a.mouse_sel = Some(crate::app::MouseSelection {
            anchor_x: 11,
            anchor_y: 6,
            focus_x: 18,
            focus_y: 7,
            dragging: true,
        });
        copy_modal_selection(&mut a, 18, 7);
        let active = a.dialogs.active().cloned();
        apply_modal_scrollbar(&mut a, active.as_ref(), 6, None);
        a.dialog_scrollbar_hit = Some(Rect {
            x: 29,
            y: 6,
            width: 1,
            height: 6,
        });
        a.dialog_list_total = 20;
        a.dialog_list_visible = 6;
        apply_modal_scrollbar(&mut a, active.as_ref(), 8, Some(1));
        let grab = scrollbar_grab_at(
            &a,
            8,
            Rect {
                x: 29,
                y: 6,
                width: 1,
                height: 6,
            },
        );
        let _ = grab;
    }

    #[test]
    fn chat_scrollbar_offset_snaps_and_noops() {
        let mut a = app();
        apply_chat_scrollbar_offset(&mut a, 5, None);
        a.chat_scrollbar_hit = Some(Rect {
            x: 40,
            y: 1,
            width: 1,
            height: 10,
        });
        a.chat_scroll_total = 0;
        apply_chat_scrollbar_offset(&mut a, 5, None);

        a.chat_scroll_total = 8;
        a.chat_viewport_rows = 10;
        apply_chat_scrollbar_offset(&mut a, 5, None);
        assert_eq!(a.scroll_offset, 0);
        assert!(a.auto_scroll);

        a.chat_scroll_total = 100;
        a.chat_viewport_rows = 10;
        apply_chat_scrollbar_offset(&mut a, 1, None);
        assert!(a.scroll_offset > 0, "top of track → oldest");
        apply_chat_scrollbar_offset(&mut a, 10, None);
        assert_eq!(a.scroll_offset, 0, "bottom of track → newest");
        apply_chat_scrollbar_offset(&mut a, 0, None);
        apply_chat_scrollbar_offset(&mut a, 50, Some(1));
        let grab = chat_scrollbar_grab_at(
            &a,
            3,
            Rect {
                x: 40,
                y: 1,
                width: 1,
                height: 10,
            },
        );
        let _ = grab;
    }

    #[test]
    fn paste_token_edits_as_a_unit_and_session_paste() {
        let mut a = app();
        a.insert_paste_text("one\ntwo\nthree\nfour");
        assert!(!a.pending_pastes.is_empty());
        a.input_cursor = 0;
        handle_event(&mut a, key(KeyCode::Delete));
        assert!(a.input_buffer.is_empty() || !a.input_buffer.contains('\n'));

        let mut a = app();
        a.insert_paste_text("one\ntwo\nthree\nfour");
        let end = a.input_buffer.len();
        a.input_cursor = end;
        handle_event(&mut a, key(KeyCode::Left));
        assert!(a.input_cursor < end);
        handle_event(&mut a, key(KeyCode::Right));
        assert_eq!(a.input_cursor, a.input_buffer.len());

        let mut a = app();
        a.mode = AppMode::Session;
        handle_event(&mut a, Event::Paste("hello session".into()));
        assert!(a.input_buffer.contains("hello session"));
    }

    #[test]
    fn provider_form_fields_and_command_aliases() {
        let mut a = app();
        open_provider_dialog(&mut a);
        a.provider_dialog.mode = crate::app::ProviderDialogMode::AddCustom;
        for (field, ch, get) in [
            (1usize, 'k', "form_api_key"),
            (2, 'u', "form_base_url"),
            (3, 'h', "form_headers"),
        ] {
            a.provider_dialog.active_field = field;
            handle_event(&mut a, key(KeyCode::Char(ch)));
            let val = match field {
                1 => a.provider_dialog.form_api_key.as_str(),
                2 => a.provider_dialog.form_base_url.as_str(),
                _ => a.provider_dialog.form_headers.as_str(),
            };
            assert_eq!(val, ch.to_string(), "{get}");
            handle_event(&mut a, key(KeyCode::Backspace));
        }

        for cmd in [":quit", ":h", ":prov"] {
            let mut a = app();
            a.mode = AppMode::Command;
            a.key_context = KeymapContext::Command;
            a.command.buffer = cmd.into();
            handle_event(&mut a, key(KeyCode::Enter));
            match cmd {
                ":quit" => assert!(!a.running),
                ":h" => assert_eq!(a.mode, AppMode::Help),
                _ => assert!(matches!(a.dialogs.active(), Some(DialogKind::Provider))),
            }
        }
    }

    #[test]
    fn open_subagent_falls_back_to_last_and_shift_jumps() {
        let mut a = app();
        a.upsert_subagent(crate::app::SubagentUpdate {
            id: "only".into(),
            kind: "explore".into(),
            description: "d".into(),
            status: "running".into(),
            activity: String::new(),
            elapsed_ms: 0,
            output: String::new(),
        });
        a.selected_msg = None;
        open_selected_subagent(&mut a);
        assert_eq!(a.open_subagent.as_deref(), Some("only"));

        let mut a = app();
        a.add_message(ChatRole::User, "u1");
        a.add_message(ChatRole::Assistant, "a1");
        a.add_message(ChatRole::User, "u2");
        handle_event(
            &mut a,
            Event::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT)),
        );
        handle_event(
            &mut a,
            Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT)),
        );
    }

    #[test]
    fn mouse_drag_without_down_and_plain_click() {
        let mut a = app();
        handle_event(&mut a, mouse(MouseEventKind::Drag(MouseButton::Left), 4, 4));
        assert!(a.mouse_sel.is_some());
        handle_event(&mut a, mouse(MouseEventKind::Up(MouseButton::Left), 4, 4));

        let mut a = app();
        handle_event(&mut a, mouse(MouseEventKind::Down(MouseButton::Left), 2, 2));
        handle_event(&mut a, mouse(MouseEventKind::Up(MouseButton::Left), 2, 2));
        assert!(a.mouse_sel.is_none());
    }

    #[test]
    fn move_in_dialog_to_clamps_each_kind() {
        let mut a = app();
        open_provider_dialog(&mut a);
        move_in_dialog_to(&mut a, &DialogKind::Provider, 99);
        assert!(a.provider_dialog.selected > 0);

        a.model_selection.models = vec![("p".into(), "m".into())];
        move_in_dialog_to(&mut a, &DialogKind::Model, 9);
        assert_eq!(a.model_selection.selected, 0);

        a.primary_agents = vec!["build".into(), "plan".into()];
        move_in_dialog_to(&mut a, &DialogKind::Agent, 9);
        assert_eq!(a.agent_picker_selected, 1);

        a.session_list.sessions = vec![crate::app::SessionEntry {
            id: "x".into(),
            title: "t".into(),
            messages: 0,
            updated_at: None,
            live: None,
        }];
        move_in_dialog_to(&mut a, &DialogKind::SessionList, 4);
        assert_eq!(a.session_list.selected, 0);

        a.sessions_rows = vec![crate::app::SessionDashboardRow {
            parked_idx: None,
            title: "s".into(),
            glyph: "·".into(),
            state_label: "idle".into(),
            preview: String::new(),
            unread: false,
        }];
        move_in_dialog_to(&mut a, &DialogKind::Sessions, 3);
        assert_eq!(a.sessions_cursor, 0);

        a.login_dialog.rows = vec![crate::app::LoginProviderRow {
            provider: "x".into(),
            label: "X".into(),
            connected: false,
        }];
        move_in_dialog_to(&mut a, &DialogKind::Login, 2);
        assert_eq!(a.login_dialog.selected, 0);

        move_in_dialog_to(&mut a, &DialogKind::Theme, 0);
        assert_eq!(a.theme_selected, 0);
        move_in_dialog_to(&mut a, &DialogKind::Help, 0);
    }

    #[test]
    fn dialog_question_keys_move_without_free_text() {
        let mut a = app();
        a.ask_question(vec![whycode_tools::question::QuestionSpec {
            prompt: "Go?".into(),
            options: vec![
                whycode_tools::question::QuestionOption {
                    label: "A".into(),
                    description: String::new(),
                    preview: None,
                },
                whycode_tools::question::QuestionOption {
                    label: "B".into(),
                    description: String::new(),
                    preview: None,
                },
            ],
            multi_select: true,
        }]);
        handle_event(&mut a, key(KeyCode::Down));
        handle_event(&mut a, key(KeyCode::Up));
        if let Some(DialogKind::Question(st)) = a.dialogs.active() {
            assert!(!st.free_text_focus);
        }
    }

    #[test]
    fn coalesce_resizes_keeps_ordered_non_resize_events_and_last_size() {
        let typed = key(KeyCode::Char('x'));
        let moved = mouse(MouseEventKind::Moved, 2, 3);
        let mut events = vec![
            Event::Resize(80, 24),
            typed.clone(),
            Event::Resize(100, 30),
            moved.clone(),
            Event::Resize(120, 40),
        ];

        coalesce_resizes(&mut events);

        assert_eq!(events, vec![typed, moved, Event::Resize(120, 40)]);
        let mut unchanged = vec![Event::FocusGained, Event::FocusLost];
        let expected = unchanged.clone();
        coalesce_resizes(&mut unchanged);
        assert_eq!(unchanged, expected);
    }

    #[test]
    fn direct_input_actions_handle_invalid_utf8_cursor_and_history_edges() {
        let mut a = app();
        a.input_buffer = "şa".into();
        a.input_cursor = 1;
        handle_input_action(
            &mut a,
            crate::keymap::Action::InputDelete,
            &KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
        );
        assert_eq!(a.input_buffer, "a", "mid-codepoint cursor clamps backward");
        assert_eq!(a.input_cursor, 0);

        a.input_history = vec!["first".into(), "second".into()];
        a.input_history_idx = 0;
        handle_input_action(
            &mut a,
            crate::keymap::Action::InputHistoryPrev,
            &KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
        );
        assert_eq!(a.input_history_idx, 0, "history does not underflow");
        a.input_history_idx = a.input_history.len();
        handle_input_action(
            &mut a,
            crate::keymap::Action::InputHistoryNext,
            &KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        );
        assert_eq!(a.input_history_idx, 2, "history does not overflow");
    }
}
