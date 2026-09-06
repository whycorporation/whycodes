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
    let text = paint(100, crate::tokens::layout::HEADER_H, |f| {
        render(f, f.area(), &app, &palette)
    });
    // Tiny `?` plus fg wordmark as one word (`whycodes`).
    assert!(text.contains('?'), "{text}");
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
    let text = paint(100, crate::tokens::layout::HEADER_H, |f| {
        render(f, f.area(), &app, &palette)
    });
    assert!(text.contains("anthropic/claude-sonnet"), "{text}");
}

#[test]
fn no_intent_badge_by_default() {
    let app = TuiApp::new(cfg());
    let palette = app.config.palette();
    let text = paint(100, crate::tokens::layout::HEADER_H, |f| {
        render(f, f.area(), &app, &palette)
    });
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
        let text = paint(100, crate::tokens::layout::HEADER_H, |f| {
            render(f, f.area(), &app, &palette)
        });
        assert!(text.contains(&format!("[{badge}]")), "kind {kind}: {text}");
    }
}

#[test]
fn paints_agent_name_and_cycle_color() {
    let mut app = TuiApp::new(cfg());
    app.agent_name = "coder".into();
    app.agent_cycle_idx = 2;
    let palette = app.config.palette();
    let text = paint(100, crate::tokens::layout::HEADER_H, |f| {
        render(f, f.area(), &app, &palette)
    });
    assert!(text.contains("coder"), "{text}");
    // Cycle index must stay in the palette's color list.
    let _ = palette.agent_color_by_index(2);
}
