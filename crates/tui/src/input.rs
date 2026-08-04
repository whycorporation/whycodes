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
        app.toasts
            .push(crate::toast::ToastKind::Success, label);
    }
    if let Some(err) = last_err {
        app.toasts
            .push(crate::toast::ToastKind::Warning, err);
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

/// Grok Esc hierarchy (steal-Esc first, then double-Esc clear).
///
/// Cancel-while-busy is owned by the run loop (has the CancelFlag); here we
/// only handle idle clear / mode exit. Slash + help still steal Esc.
fn handle_escape(app: &mut TuiApp) {
    // 1. Steal: slash suggest
    if app.slash_suggest.active {
        app.slash_suggest.dismiss();
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
        app.toasts.push(
            crate::toast::ToastKind::Info,
            "press Esc again to clear",
        );
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
            if app.input_cursor == 0 && app.input_buffer.is_empty() && !app.pending_images.is_empty() =>
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
    app.mouse_pos = Some((mouse.column, mouse.row));

    match mouse.kind {
        MouseEventKind::ScrollDown => {
            app.scroll_rows(-3);
        }
        MouseEventKind::ScrollUp => {
            app.scroll_rows(3);
        }
        MouseEventKind::Moved => {
            // Position already updated above; next frame paints hover %.
            return true;
        }
        MouseEventKind::Down(MouseButton::Left) => {
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
            if let Some(sel) = &mut app.mouse_sel {
                sel.focus_x = mouse.column;
                sel.focus_y = mouse.row;
                sel.dragging = false;
                // Linear selection needs the real endpoints, not a normalized
                // rectangle (that would pull in the empty corner of short lines).
                let same_cell =
                    sel.anchor_x == sel.focus_x && sel.anchor_y == sel.focus_y;
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

// ── Dialog Key Handling ────────────────────────────────────────────────
fn handle_dialog_key(app: &mut TuiApp, key: &KeyEvent) -> bool {
    let action =
        crate::keymap::Keymap::new().resolve(KeymapContext::Dialog, app.focus, key);

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
            app.dialogs.pop();
            if !app.dialogs.is_open() {
                app.mode = AppMode::Normal;
                app.key_context = KeymapContext::Normal;
            }
        }
        Some(Action::DialogConfirm) => {
            confirm_dialog(app, &active);
        }
        // Up/Down are bound to next/prev field. In a form that means the next
        // input; in a list dialog it means the next row.
        Some(Action::DialogNextField) => move_in_dialog(app, &active, 1),
        Some(Action::DialogPrevField) => move_in_dialog(app, &active, -1),
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
        _ => {}
    }
    app.dialogs.pop();
    if !app.dialogs.is_open() {
        app.mode = AppMode::Normal;
        app.key_context = KeymapContext::Normal;
    }
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
    if let Ok(config) = whycode_core::config::Config::load() {
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
        _ => {}
    }
}
