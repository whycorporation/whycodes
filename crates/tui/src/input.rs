// ── input.rs: Input handler ────────────────────────────────────────────
// Translates raw crossterm events into actions via the keymap,
// then updates application state accordingly.

use crate::app::{AppMode, ConfirmAction, DialogKind, TuiApp};
use crate::keymap::{Action, KeymapContext};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, MouseEvent, MouseEventKind};

/// Process a single crossterm event and update application state.
/// Returns `false` when the app should exit.
pub fn handle_event(app: &mut TuiApp, event: Event) -> bool {
    match event {
        Event::Key(key) => handle_key(app, key),
        Event::Mouse(mouse) => handle_mouse(app, mouse),
        Event::Resize(_, _) => true,
        _ => true,
    }
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

    // Resolve and dispatch.
    let action = crate::keymap::Keymap::new().resolve(ctx, &key);

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
            match app.mode {
                AppMode::Command => {
                    app.mode = AppMode::Normal;
                    app.key_context = KeymapContext::Normal;
                    app.command.buffer.clear();
                }
                AppMode::Help => {
                    app.mode = AppMode::Normal;
                    app.key_context = KeymapContext::Normal;
                }
                _ => {}
            }
            true
        }
        Some(Action::SubmitInput) => {
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
            // TODO: populate models from config
            app.mode = AppMode::Dialog;
            app.key_context = KeymapContext::Dialog;
            app.dialogs.push(DialogKind::Model);
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
            // Handled in run loop (needs Agent + Config); mark status only here.
            app.status_message = "Tab: switch agent (run loop)".into();
            true
        }
        Some(Action::ScrollUp) => {
            app.auto_scroll = false;
            if app.scroll_offset < app.messages.len().saturating_sub(1) {
                app.scroll_offset += 1;
            }
            true
        }
        Some(Action::ScrollDown) => {
            app.scroll_offset = app.scroll_offset.saturating_sub(1);
            if app.scroll_offset == 0 {
                app.auto_scroll = true;
            }
            true
        }
        Some(Action::ScrollPageUp) => {
            app.auto_scroll = false;
            app.scroll_offset = (app.scroll_offset + 10).min(app.messages.len().saturating_sub(1));
            true
        }
        Some(Action::ScrollPageDown) => {
            app.scroll_offset = app.scroll_offset.saturating_sub(10);
            if app.scroll_offset == 0 {
                app.auto_scroll = true;
            }
            true
        }
        Some(Action::ScrollToTop) => {
            app.auto_scroll = false;
            app.scroll_offset = app.messages.len().saturating_sub(1);
            true
        }
        Some(Action::ScrollToBottom) => {
            app.scroll_offset = 0;
            app.auto_scroll = true;
            true
        }
        // Input editing actions
        Some(action) => {
            handle_input_action(app, action, &key);
            true
        }
        None => {
            // Unmapped key — treat as text input in command/normal modes.
            match app.mode {
                AppMode::Normal => {
                    if let KeyCode::Char(c) = key.code {
                        app.input_buffer.insert(app.input_cursor, c);
                        app.input_cursor += 1;
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

fn handle_input_action(app: &mut TuiApp, action: Action, _key: &KeyEvent) {
    match action {
        Action::InputBackspace if app.input_cursor > 0 => {
            app.input_cursor -= 1;
            app.input_buffer.remove(app.input_cursor);
        }
        Action::InputDelete if app.input_cursor < app.input_buffer.len() => {
            app.input_buffer.remove(app.input_cursor);
        }
        Action::InputLeft => {
            app.input_cursor = app.input_cursor.saturating_sub(1);
        }
        Action::InputRight if app.input_cursor < app.input_buffer.len() => {
            app.input_cursor += 1;
        }
        Action::InputHome => {
            app.input_cursor = 0;
        }
        Action::InputEnd => {
            app.input_cursor = app.input_buffer.len();
        }
        Action::InputClear => {
            app.input_buffer.clear();
            app.input_cursor = 0;
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

fn handle_mouse(app: &mut TuiApp, mouse: MouseEvent) -> bool {
    match mouse.kind {
        MouseEventKind::ScrollDown => {
            app.scroll_offset = app.scroll_offset.saturating_sub(1);
        }
        MouseEventKind::ScrollUp if app.scroll_offset < app.messages.len().saturating_sub(1) => {
            app.scroll_offset += 1;
        }
        _ => {}
    }
    true
}

// ── Dialog Key Handling ────────────────────────────────────────────────
fn handle_dialog_key(app: &mut TuiApp, key: &KeyEvent) -> bool {
    let action = crate::keymap::Keymap::new().resolve(KeymapContext::Dialog, key);

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
        Some(Action::DialogNextField) => {
            if matches!(active, DialogKind::Provider) {
                app.provider_dialog.active_field = (app.provider_dialog.active_field + 1) % 4;
            }
        }
        Some(Action::DialogPrevField) => {
            if matches!(active, DialogKind::Provider) {
                app.provider_dialog.active_field = if app.provider_dialog.active_field == 0 {
                    3
                } else {
                    app.provider_dialog.active_field - 1
                };
            }
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
            // Forward char input to provider form fields.
            if matches!(active, DialogKind::Provider)
                && let KeyCode::Char(c) = key.code
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
