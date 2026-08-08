// ── ui/dialogs/mod.rs: Dialog rendering system ────────────────────────
// Renders modal overlays: provider, model, help, confirm, alert.
// Chrome is Grok-style (ModalWindow) — see `base::dialog_frame`.

mod alert;
mod base;
mod confirm;
mod help;
mod provider;
mod question;
mod select;

pub use alert::*;
pub use base::*;
pub use confirm::*;
pub use help::*;
pub use provider::*;
pub use question::*;
pub use select::*;

use crate::app::TuiApp;
use crate::theme::ThemePalette;
use ratatui::Frame;

/// Main dialog render — dispatches to the correct dialog renderer
/// based on the active dialog kind.
pub fn render(frame: &mut Frame, app: &mut TuiApp, palette: &ThemePalette) {
    app.clear_dialog_hits();
    let active = match app.dialogs.active().cloned() {
        Some(d) => d,
        None => return,
    };

    let mouse = app.mouse_pos;
    match active {
        crate::app::DialogKind::Provider => render_provider_dialog(frame, app, palette),
        crate::app::DialogKind::Model => render_model_dialog(frame, app, palette),
        crate::app::DialogKind::Help => render_help_overlay(frame, app, palette),
        crate::app::DialogKind::Confirm { title, message, .. } => {
            let chrome = render_confirm_dialog(frame, &title, &message, palette, mouse);
            app.apply_modal_chrome(chrome.close_hit, chrome.modal, None);
        }
        crate::app::DialogKind::Alert { title, message } => {
            let chrome = render_alert_dialog(frame, &title, &message, palette, mouse);
            app.apply_modal_chrome(chrome.close_hit, chrome.modal, None);
        }
        crate::app::DialogKind::Permission { tool_name, detail } => {
            let chrome = render_permission_dialog(frame, &tool_name, &detail, palette, mouse);
            app.apply_modal_chrome(chrome.close_hit, chrome.modal, None);
        }
        crate::app::DialogKind::Question(state) => {
            let paint = render_question_dialog(frame, &state, palette, mouse);
            // One row per option so mouse click maps to option index.
            app.apply_select_paint(
                paint.chrome.close_hit,
                paint.list_area,
                None,
                0,
                paint.list_total,
                paint.list_total,
                Some(paint.chrome.modal),
            );
        }
        crate::app::DialogKind::SessionList => {
            let items: Vec<SelectItem> = app
                .session_list
                .sessions
                .iter()
                .map(|s| {
                    let short: String = s.id.chars().take(8).collect();
                    SelectItem::with_detail(
                        s.title.clone(),
                        format!("{short} · {} messages", s.messages),
                    )
                })
                .collect();
            let selected = app.session_list.selected;
            let info = render_select(
                frame,
                " Sessions  ·  Enter to resume ",
                &items,
                selected,
                "No sessions yet — they are recorded as you use whycode.",
                palette,
                mouse,
            );
            app.apply_select_paint(
                info.close_hit,
                info.list_area,
                info.scrollbar_hit,
                info.scroll_start,
                info.visible,
                info.total,
                info.modal,
            );
        }
        crate::app::DialogKind::Theme => {
            use crate::theme::ThemeName;
            let items: Vec<SelectItem> = ThemeName::ALL
                .iter()
                .map(|t| {
                    let mark = if *t == app.theme { " · current" } else { "" };
                    SelectItem::with_detail(t.name().to_string(), format!("built-in{mark}"))
                })
                .collect();
            let selected = app.theme_selected.min(items.len().saturating_sub(1));
            let info = render_select(
                frame,
                " Themes  ·  Enter to apply ",
                &items,
                selected,
                "No themes.",
                palette,
                mouse,
            );
            app.apply_select_paint(
                info.close_hit,
                info.list_area,
                info.scrollbar_hit,
                info.scroll_start,
                info.visible,
                info.total,
                info.modal,
            );
        }
        crate::app::DialogKind::Sessions => {
            let items: Vec<SelectItem> = app
                .sessions_rows
                .iter()
                .map(|r| {
                    let unread = if r.unread { " ●" } else { "" };
                    SelectItem::with_detail(
                        format!("{} {}{}", r.glyph, r.title, unread),
                        format!("{} · {}", r.state_label, r.preview),
                    )
                })
                .collect();
            let selected = app.sessions_cursor.min(items.len().saturating_sub(1));
            let info = render_select(
                frame,
                " Live sessions  ·  Enter switch  ·  Ctrl+N new ",
                &items,
                selected,
                "One session — Ctrl+N opens another.",
                palette,
                mouse,
            );
            app.apply_select_paint(
                info.close_hit,
                info.list_area,
                info.scrollbar_hit,
                info.scroll_start,
                info.visible,
                info.total,
                info.modal,
            );
        }
        _ => {}
    }
}

/// Standalone help overlay (not stack-based, triggered by `?` key).
///
/// Clears and repaints modal hits so selection/copy match other popups.
pub fn render_help(frame: &mut Frame, app: &mut TuiApp, palette: &ThemePalette) {
    app.clear_dialog_hits();
    render_help_overlay(frame, app, palette);
}
