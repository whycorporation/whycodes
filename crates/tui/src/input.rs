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
        Event::Resize(_, _) => true,
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
    // Insert remaining text at the cursor (same path as typed chars).
    let pos = clamp_cursor(&app.input_buffer, app.input_cursor);
    app.input_buffer.insert_str(pos, &text);
    app.input_cursor = pos + text.len();
    app.slash_suggest.refresh(&app.input_buffer);
    app.esc_armed_at = None;
}

fn handle_key(app: &mut TuiApp, key: KeyEvent) -> bool {
    // Windows (and enhanced keyboard) emits Press + Release for every key.
    // Without this filter each character is inserted twice.
    if key.kind != KeyEventKind::Press && key.kind != KeyEventKind::Repeat {
        return true;
    }

    let ctx = app.key_context;

    // If dialog mode, route to dialog handler first.
    if ctx == KeymapContext::Dialog {
        return handle_dialog_key(app, &key);
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
        Some(Action::ToggleHelp) => {
            if app.mode == AppMode::Help {
                app.mode = AppMode::Normal;
                app.key_context = KeymapContext::Normal;
            } else {
                app.mode = AppMode::Help;
                app.key_context = KeymapContext::Help;
                app.help_scroll = 0;
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
        // Slash-suggest: Tab completes; Up/Down navigate the list.
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
            true
        }
        Some(Action::OpenProviderDialog) => {
            open_provider_dialog(app);
            true
        }
        Some(Action::OpenModelDialog) => {
            open_dialog(app, DialogKind::Model);
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
                        app.esc_armed_at = None;
                    }
                }
                AppMode::Command => {
                    if let KeyCode::Char(c) = key.code {
                        app.command.buffer.push(c);
                    }
                }
                AppMode::Help => {
                    if let KeyCode::Char('q') = key.code {
                        app.mode = AppMode::Normal;
                        app.key_context = KeymapContext::Normal;
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
fn handle_escape(app: &mut TuiApp) {
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
    match app.mode {
        AppMode::Command => {
            app.mode = AppMode::Normal;
            app.key_context = KeymapContext::Normal;
            app.command.buffer.clear();
            app.esc_armed_at = None;
            return;
        }
        AppMode::Help => {
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
            let start = prev_boundary(&app.input_buffer, end);
            app.input_buffer.replace_range(start..end, "");
            app.input_cursor = start;
            app.slash_suggest.refresh(&app.input_buffer);
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
            let end = next_boundary(&app.input_buffer, start);
            app.input_buffer.replace_range(start..end, "");
            app.input_cursor = start;
            app.slash_suggest.refresh(&app.input_buffer);
        }
        Action::InputClear => {
            app.input_buffer.clear();
            app.input_cursor = 0;
            app.pending_images.clear();
            app.slash_suggest.dismiss();
        }
        Action::InputLeft => {
            app.input_cursor = prev_boundary(&app.input_buffer, app.input_cursor);
        }
        Action::InputRight if app.input_cursor < app.input_buffer.len() => {
            app.input_cursor = next_boundary(&app.input_buffer, app.input_cursor);
        }
        Action::InputHome => {
            app.input_cursor = 0;
        }
        Action::InputEnd => {
            app.input_cursor = app.input_buffer.len();
        }
        Action::InputNewline => {
            let pos = clamp_cursor(&app.input_buffer, app.input_cursor);
            app.input_buffer.insert(pos, '\n');
            app.input_cursor = pos + 1;
            app.slash_suggest.dismiss();
        }
        Action::InputHistoryPrev if !app.input_history.is_empty() && app.input_history_idx > 0 => {
            app.input_history_idx -= 1;
            app.input_buffer = app.input_history[app.input_history_idx].clone();
            app.input_cursor = app.input_buffer.len();
        }
        Action::InputHistoryNext if app.input_history_idx < app.input_history.len() => {
            app.input_history_idx += 1;
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
            // Turn-status [stop] → cancel current agent turn.
            if app.turn_stop_hit.contains(mouse.column, mouse.row) && app.is_busy() {
                app.pending_cancel = true;
                app.status_message = "Cancelling…".into();
                app.mark_dirty();
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
                    DialogKind::Model => {
                        app.model_selection.selected = idx;
                        confirm_dialog(app, active);
                    }
                    DialogKind::Theme => {
                        app.theme_selected = idx;
                        confirm_dialog(app, active);
                    }
                    DialogKind::Provider
                        if app.provider_dialog.mode == crate::app::ProviderDialogMode::Select =>
                    {
                        app.provider_dialog.selected = idx;
                        confirm_dialog(app, active);
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
        crate::clipboard::text_from_cells(
            &app.screen_cells,
            sel.anchor_x,
            sel.anchor_y,
            fx,
            fy,
        )
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

/// Pop the active dialog and leave dialog mode when the stack is empty.
fn dismiss_dialog(app: &mut TuiApp) {
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
        DialogKind::SessionList => {
            let len = app.session_list.sessions.len();
            if len > 0 {
                app.session_list.selected = idx.min(len - 1);
            }
        }
        DialogKind::Theme => {
            let len = crate::theme::ThemeName::ALL.len();
            if len > 0 {
                app.theme_selected = idx.min(len - 1);
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

    match action {
        Some(Action::DialogCancel) => {
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
        DialogKind::SessionList => {
            if let Some(entry) = app.session_list.sessions.get(app.session_list.selected) {
                app.pending_session_id = Some(entry.id.clone());
            }
        }
        DialogKind::Theme => {
            use crate::theme::ThemeName;
            if let Some(t) = ThemeName::ALL.get(app.theme_selected).copied() {
                app.theme = t;
                app.config.theme = t;
                // Drop file-override so the built-in palette is visible immediately.
                app.config.theme_override = None;
                app.status_message = format!("Theme → {}", t.name());
                app.toasts
                    .push(crate::toast::ToastKind::Success, format!("Theme · {}", t.name()));
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
            app.mode = AppMode::Help;
            app.key_context = KeymapContext::Help;
        }
        Some(":provider") | Some(":prov") => {
            open_provider_dialog(app);
        }
        Some(":model") => {
            app.mode = AppMode::Dialog;
            app.key_context = KeymapContext::Dialog;
            app.dialogs.push(DialogKind::Model);
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
        DialogKind::SessionList => {
            app.session_list.selected = move_selection(
                app.session_list.selected,
                app.session_list.sessions.len(),
                delta,
            );
        }
        DialogKind::Theme => {
            app.theme_selected = move_selection(
                app.theme_selected,
                crate::theme::ThemeName::ALL.len(),
                delta,
            );
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
}
