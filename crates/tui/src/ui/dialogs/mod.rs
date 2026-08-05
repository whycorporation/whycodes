// ── ui/dialogs/mod.rs: Dialog rendering system ────────────────────────
// Renders modal overlays: provider, model, help, confirm, alert.
// Chrome is Grok-style (ModalWindow) — see `base::dialog_frame`.

mod alert;
mod base;
mod confirm;
mod help;
mod provider;
mod select;

pub use alert::*;
pub use base::*;
pub use confirm::*;
pub use help::*;
pub use provider::*;
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
            app.dialog_close_hit = chrome.close_hit;
        }
        crate::app::DialogKind::Alert { title, message } => {
            let chrome = render_alert_dialog(frame, &title, &message, palette, mouse);
            app.dialog_close_hit = chrome.close_hit;
        }
        crate::app::DialogKind::Permission { tool_name, detail } => {
            let title = format!("Permission: {tool_name}");
            let message = format!("{detail}\n\n[y/a] Allow   [n/d/Esc] Deny");
            let chrome = render_confirm_dialog(frame, &title, &message, palette, mouse);
            app.dialog_close_hit = chrome.close_hit;
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
            );
        }
        _ => {}
    }
}

/// Standalone help overlay (not stack-based, triggered by `?` key).
pub fn render_help(frame: &mut Frame, app: &TuiApp, palette: &ThemePalette) {
    render_help_overlay(frame, app, palette);
}
