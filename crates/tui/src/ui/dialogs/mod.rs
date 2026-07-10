// ── ui/dialogs/mod.rs: Dialog rendering system ────────────────────────
// Renders modal overlays: provider, model, help, confirm, alert.

mod base;
mod provider;
mod help;
mod confirm;
mod alert;

pub use base::*;
pub use provider::*;
pub use help::*;
pub use confirm::*;
pub use alert::*;

use ratatui::Frame;
use crate::app::TuiApp;
use crate::theme::ThemePalette;

/// Main dialog render — dispatches to the correct dialog renderer
/// based on the active dialog kind.
pub fn render(frame: &mut Frame, app: &TuiApp, palette: &ThemePalette) {
    let active = match app.dialogs.active() {
        Some(d) => d,
        None => return,
    };

    match active {
        crate::app::DialogKind::Provider => render_provider_dialog(frame, app, palette),
        crate::app::DialogKind::Model => render_model_dialog(frame, app, palette),
        crate::app::DialogKind::Help => render_help_overlay(frame, app, palette),
        crate::app::DialogKind::Confirm { title, message, .. } => {
            render_confirm_dialog(frame, title, message, palette)
        }
        crate::app::DialogKind::Alert { title, message } => {
            render_alert_dialog(frame, title, message, palette)
        }
        _ => {}
    }
}

/// Standalone help overlay (not stack-based, triggered by `?` key).
pub fn render_help(frame: &mut Frame, app: &TuiApp, palette: &ThemePalette) {
    render_help_overlay(frame, app, palette);
}
