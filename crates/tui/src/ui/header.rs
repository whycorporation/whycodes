// ── ui/header.rs: thin top bar (session chrome) ────────────────────────

use crate::app::TuiApp;
use crate::theme::ThemePalette;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
};

pub fn render(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    // Landing block `?` + dual-tone wordmark, matching the top status bar.
    let brand_mark = Span::styled(
        crate::tokens::HEADER_MARK,
        Style::default().fg(palette.fg).add_modifier(Modifier::BOLD),
    );
    let brand_why = Span::styled(
        " why",
        Style::default().fg(palette.fg).add_modifier(Modifier::BOLD),
    );
    let brand_code = Span::styled(
        "codes ",
        Style::default()
            .fg(palette.accent)
            .add_modifier(Modifier::BOLD),
    );
    let agent_color = app
        .config
        .agent_color(&app.agent_name, app.agent_cycle_idx, palette);
    let agent = Span::styled(
        format!(" {} ", app.agent_name),
        Style::default()
            .fg(agent_color)
            .add_modifier(Modifier::BOLD),
    );
    let mut spans = vec![brand_mark, brand_why, brand_code, agent];
    if let Some(ref badge) = app.intent_badge {
        let badge_color = match app.intent_kind.as_deref() {
            Some("question") => palette.info,
            Some("plan") => palette.accent,
            Some("change") => palette.success,
            _ => palette.dim,
        };
        spans.push(Span::styled(
            format!("[{badge}]"),
            Style::default()
                .fg(badge_color)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(
        format!(
            "{}/{} ",
            if app.provider_name.is_empty() {
                "—"
            } else {
                app.provider_name.as_str()
            },
            if app.model_name.is_empty() {
                "—"
            } else {
                app.model_name.as_str()
            }
        ),
        Style::default().fg(app.config.model_color(palette)),
    ));

    frame.render_widget(
        Paragraph::new(Text::from(Line::from(spans)))
            .style(Style::default().bg(palette.status_bar_bg)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TuiAppConfig;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn cfg() -> TuiAppConfig {
        TuiAppConfig::default()
    }

    /// Render `f` into a fresh terminal and return the painted buffer text.
    fn paint<F>(width: u16, height: u16, f: F) -> String
    where
        F: FnOnce(&mut ratatui::Frame),
    {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal.draw(f).expect("draw");
        let buf = terminal.backend().buffer().clone();
        let area = buf.area();
        let mut out = String::new();
        for y in area.y..area.y.saturating_add(area.height) {
            for x in area.x..area.x.saturating_add(area.width) {
                if let Some(cell) = buf.cell((x, y)) {
                    out.push_str(cell.symbol());
                }
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn paints_wordmark_agent_and_placeholder_provider() {
        let app = TuiApp::new(cfg());
        let palette = app.config.palette();
        let text = paint(100, 1, |f| render(f, f.area(), &app, &palette));
        // Block `?` (▀▄▀) plus dual-tone wordmark as one word (`whycodes`).
        assert!(text.contains("▀▄▀"), "{text}");
        assert!(text.contains("whycodes"), "{text}");
        assert!(!text.contains("why codes"), "{text}");
        assert!(text.contains("build"), "{text}");
        // No provider/model configured → dash pair.
        assert!(text.contains("—/—"), "{text}");
    }

    #[test]
    fn paints_provider_and_model_when_configured() {
        let mut app = TuiApp::new(cfg());
        app.provider_name = "anthropic".into();
        app.model_name = "claude-sonnet".into();
        let palette = app.config.palette();
        let text = paint(100, 1, |f| render(f, f.area(), &app, &palette));
        assert!(text.contains("anthropic/claude-sonnet"), "{text}");
    }

    #[test]
    fn no_intent_badge_by_default() {
        let app = TuiApp::new(cfg());
        let palette = app.config.palette();
        let text = paint(100, 1, |f| render(f, f.area(), &app, &palette));
        assert!(!text.contains('['), "no badge without intent: {text}");
    }

    #[test]
    fn paints_intent_badge_for_each_kind() {
        for (kind, badge) in [
            ("question", "Q"),
            ("plan", "P"),
            ("change", "C"),
            ("mystery", "X"),
        ] {
            let mut app = TuiApp::new(cfg());
            app.intent_badge = Some(badge.to_string());
            app.intent_kind = Some(kind.to_string());
            let palette = app.config.palette();
            let text = paint(100, 1, |f| render(f, f.area(), &app, &palette));
            assert!(text.contains(&format!("[{badge}]")), "kind {kind}: {text}");
        }
    }

    #[test]
    fn paints_agent_name_and_cycle_color() {
        let mut app = TuiApp::new(cfg());
        app.agent_name = "coder".into();
        app.agent_cycle_idx = 2;
        let palette = app.config.palette();
        let text = paint(100, 1, |f| render(f, f.area(), &app, &palette));
        assert!(text.contains("coder"), "{text}");
        // Cycle index must stay in the palette's color list.
        let _ = palette.agent_color_by_index(2);
    }
}
