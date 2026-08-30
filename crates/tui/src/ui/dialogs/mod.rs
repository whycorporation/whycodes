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

#[cfg(test)]
#[path = "render_tests.rs"]
mod render_tests;

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
        crate::app::DialogKind::Agent => {
            let items: Vec<SelectItem> = app
                .primary_agents
                .iter()
                .map(|name| {
                    let mark = if name == &app.agent_name {
                        " · current"
                    } else {
                        ""
                    };
                    SelectItem::with_detail(name.clone(), format!("agent{mark}"))
                })
                .collect();
            let selected = app.agent_picker_selected.min(items.len().saturating_sub(1));
            let info = render_select(
                frame,
                " Agent  ·  Enter to switch ",
                &items,
                selected,
                "No agents configured.",
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
                    let label = if s.live.is_some() {
                        format!("▸ {}", s.title)
                    } else {
                        s.title.clone()
                    };
                    SelectItem::with_detail(label, session_list_detail(s))
                })
                .collect();
            let selected = app.session_list.selected;
            let info = render_select(
                frame,
                " Resume  ·  Enter open  ·  type to filter  ·  Ctrl+W close live ",
                &items,
                selected,
                "No sessions yet — they are recorded as you use whycodes.",
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
        crate::app::DialogKind::Effort => {
            let levels = whycodes_llm::ThinkingConfig::supported_efforts(
                &app.provider_name,
                &app.model_name,
            );
            let current = whycodes_llm::ThinkingConfig::resolve_effort(
                &app.provider_name,
                &app.model_name,
                app.reasoning_effort.as_deref(),
            );
            let items: Vec<SelectItem> = levels
                .iter()
                .map(|e| {
                    let mark = if current == Some(*e) {
                        " · current"
                    } else {
                        ""
                    };
                    SelectItem::with_detail(
                        format!("{}{mark}", e.label()),
                        e.description().to_string(),
                    )
                })
                .collect();
            let selected = app
                .effort_picker_selected
                .min(items.len().saturating_sub(1));
            let info = render_select(
                frame,
                " Reasoning effort  ·  Enter to apply ",
                &items,
                selected,
                "This model has no reasoning-effort levels.",
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
        crate::app::DialogKind::ApprovalMode => {
            let items: Vec<SelectItem> = whycodes_core::types::ApprovalMode::ALL
                .iter()
                .map(|m| {
                    let mark = if *m == app.approval_mode {
                        " · current"
                    } else {
                        ""
                    };
                    SelectItem::with_detail(
                        format!("{}{mark}", m.label()),
                        m.description().to_string(),
                    )
                })
                .collect();
            let selected = app
                .approval_picker_selected
                .min(items.len().saturating_sub(1));
            let info = render_select(
                frame,
                " Approval mode  ·  Enter to apply ",
                &items,
                selected,
                "No approval modes.",
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
        crate::app::DialogKind::Login => {
            let items: Vec<SelectItem> = app
                .login_dialog
                .rows
                .iter()
                .map(|r| {
                    let status = if r.connected {
                        "✓ connected"
                    } else {
                        "not connected"
                    };
                    SelectItem::with_detail(r.label.clone(), format!("{} · {status}", r.provider))
                })
                .collect();
            let selected = app.login_dialog.selected.min(items.len().saturating_sub(1));
            let info = render_select(
                frame,
                " Sign in  ·  Enter starts OAuth login ",
                &items,
                selected,
                "No OAuth providers available.",
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

/// Session picker subtitle — Grok `/resume` row: relative clock + message count.
fn session_list_detail(s: &crate::app::SessionEntry) -> String {
    let msgs = format!("{} messages", s.messages);
    match s.updated_at {
        Some(ts) => format!("{} · {msgs}", crate::ui::timefmt::format_relative(ts)),
        None => msgs,
    }
}

/// Standalone help overlay (not stack-based, triggered by `/help`).
///
/// Clears and repaints modal hits so selection/copy match other popups.
pub fn render_help(frame: &mut Frame, app: &mut TuiApp, palette: &ThemePalette) {
    app.clear_dialog_hits();
    render_help_overlay(frame, app, palette);
}

#[cfg(test)]
mod session_when_tests {
    use super::*;

    #[test]
    fn detail_omits_time_when_timestamp_is_missing() {
        let entry = crate::app::SessionEntry {
            id: "abcdef12-9999".into(),
            title: "Fix webhook".into(),
            messages: 12,
            updated_at: None,
            live: None,
        };
        assert_eq!(session_list_detail(&entry), "12 messages");
    }

    #[test]
    fn detail_puts_time_next_to_the_message_count() {
        let entry = crate::app::SessionEntry {
            id: "abcdef12-9999".into(),
            title: "Fix webhook".into(),
            messages: 12,
            updated_at: Some(chrono::Utc::now()),
            live: None,
        };
        let detail = session_list_detail(&entry);
        assert!(
            detail.ends_with(" · 12 messages"),
            "detail {detail:?} should keep the count after the time"
        );
        assert!(
            detail.contains("just now") || detail.contains("m ago") || detail.contains("h ago"),
            "detail {detail:?} should use a Grok-style relative clock"
        );
    }
}
