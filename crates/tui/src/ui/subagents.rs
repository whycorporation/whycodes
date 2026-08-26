//! Grok-style subagent chrome: top strip, tasks-pane rows, framed child view.

use crate::app::TuiApp;
use crate::theme::ThemePalette;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

pub const SPIN: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Rows reserved under the header when any child is tracked.
pub fn strip_height(app: &TuiApp) -> u16 {
    if app.has_subagent_strip() { 1 } else { 0 }
}

pub fn render_strip(
    frame: &mut Frame,
    area: Rect,
    app: &mut TuiApp,
    palette: &ThemePalette,
    side: u16,
) {
    app.subagent_strip_hit.clear();
    if area.height == 0 || app.subagents.is_empty() {
        return;
    }
    let running = app.running_subagent_count();
    let spin = SPIN[app.spinner_frame % SPIN.len()];
    let count = if running > 0 {
        format!(
            "{spin} {running} subagent{}",
            if running == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "◇ {} subagent{}",
            app.subagents.len(),
            if app.subagents.len() == 1 { "" } else { "s" }
        )
    };

    let focus = app
        .subagents
        .iter()
        .rev()
        .find(|s| s.is_running())
        .or_else(|| app.subagents.last());

    // The count badge starts exactly on the shared body column (no hidden
    // leading space, or the strip sits one col right of the chat text).
    let mut spans = Vec::new();
    if side > 0 {
        spans.push(Span::raw(" ".repeat(side as usize)));
    }
    spans.push(Span::styled(
        format!("{count} "),
        Style::default()
            .fg(if running > 0 {
                palette.accent
            } else {
                palette.dim
            })
            .add_modifier(Modifier::BOLD),
    ));
    if let Some(row) = focus {
        spans.push(Span::styled("· ", Style::default().fg(palette.dim)));
        spans.push(Span::styled(
            format!("{} \"{}\"", row.kind, truncate(&row.description, 40)),
            Style::default().fg(palette.fg),
        ));
        if row.is_running() && !row.activity.is_empty() {
            spans.push(Span::styled(
                format!(" — {}", row.activity),
                Style::default().fg(palette.dim),
            ));
        }
        spans.push(Span::styled("  Ctrl+G", Style::default().fg(palette.dim)));
        // Hit target stays full-width even though the label is indented.
        app.subagent_strip_hit.push((area, row.id.clone()));
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(palette.status_bar_bg)),
        area,
    );
}

pub fn agent_lines(app: &TuiApp, palette: &ThemePalette) -> Vec<Line<'static>> {
    if app.subagents.is_empty() {
        return vec![Line::from(Span::styled(
            " No subagents ",
            Style::default().fg(palette.dim),
        ))];
    }
    let mut lines = vec![Line::from(Span::styled(
        " Subagents ",
        Style::default()
            .fg(palette.accent)
            .add_modifier(Modifier::BOLD),
    ))];
    for row in &app.subagents {
        let bullet = row.bullet(app.spinner_frame);
        let elapsed = if row.elapsed_ms > 0 {
            format!(" {:.1}s", row.elapsed_ms as f64 / 1000.0)
        } else {
            String::new()
        };
        lines.push(Line::from(Span::styled(
            format!(
                " {bullet} {} · {}{elapsed}",
                row.kind,
                truncate(&row.description, 28)
            ),
            Style::default().fg(if row.status == "failed" {
                palette.error
            } else if row.is_running() {
                palette.accent
            } else {
                palette.fg
            }),
        )));
    }
    lines.push(Line::from(Span::styled(
        " Enter / click strip to inspect ",
        Style::default().fg(palette.dim),
    )));
    lines
}

/// Fullscreen framed child transcript (Grok subagent view).
pub fn render_frame(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    let Some(id) = app.open_subagent.as_deref() else {
        return;
    };
    let Some(row) = app.subagents.iter().find(|s| s.id == id) else {
        return;
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette.accent))
        .title(format!(
            " {} {} · {}  [q]",
            row.bullet(app.spinner_frame),
            row.kind,
            truncate(&row.description, 40)
        ))
        .style(Style::default().bg(palette.bg));
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);

    let mut body = row.headline();
    if !row.output.is_empty() {
        body.push_str("\n\n");
        body.push_str(&row.output);
    } else if row.is_running() {
        body.push_str("\n\n(running — output appears when the child finishes)");
    }
    frame.render_widget(
        Paragraph::new(Text::from(body))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(palette.fg)),
        inner,
    );
}

fn truncate(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        s.to_string()
    } else {
        format!(
            "{}…",
            s.chars().take(max.saturating_sub(1)).collect::<String>()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::TuiApp;
    use crate::config::TuiAppConfig;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn paint(app: &mut TuiApp, w: u16, h: u16) -> String {
        let palette = app.config.palette();
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).expect("term");
        terminal
            .draw(|f| render_strip(f, f.area(), app, &palette, 0))
            .expect("draw");
        let buf = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buf.area().height {
            for x in 0..buf.area().width {
                if let Some(c) = buf.cell((x, y)) {
                    out.push_str(c.symbol());
                }
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn strip_empty_when_no_children() {
        let mut app = TuiApp::new(TuiAppConfig::default());
        assert_eq!(strip_height(&app), 0);
        let text = paint(&mut app, 80, 1);
        assert!(!text.contains("subagent"), "{text}");
    }

    #[test]
    fn strip_shows_running_count_and_kind() {
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.upsert_subagent(crate::app::SubagentUpdate {
            id: "task-1".into(),
            kind: "explore".into(),
            description: "scan the crate".into(),
            status: "running".into(),
            activity: "Thinking".into(),
            elapsed_ms: 0,
            output: String::new(),
        });
        assert_eq!(strip_height(&app), 1);
        assert_eq!(app.running_subagent_count(), 1);
        let text = paint(&mut app, 80, 1);
        assert!(text.contains("1 subagent"), "{text}");
        assert!(text.contains("explore"), "{text}");
        assert!(text.contains("scan the crate"), "{text}");
        assert!(text.contains("Ctrl+G"), "{text}");
    }

    #[test]
    fn strip_indents_text_by_side_and_keeps_full_width_hit() {
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.upsert_subagent(crate::app::SubagentUpdate {
            id: "task-1".into(),
            kind: "explore".into(),
            description: "scan the crate".into(),
            status: "running".into(),
            activity: "Thinking".into(),
            elapsed_ms: 0,
            output: String::new(),
        });
        let palette = app.config.palette();
        let backend = TestBackend::new(80, 1);
        let mut terminal = Terminal::new(backend).expect("term");
        terminal
            .draw(|f| render_strip(f, f.area(), &mut app, &palette, 2))
            .expect("draw");
        let buf = terminal.backend().buffer().clone();
        let mut row = String::new();
        for x in 0..buf.area().width {
            if let Some(c) = buf.cell((x, 0)) {
                row.push_str(c.symbol());
            }
        }
        assert!(row.starts_with("  "), "side indent: {row:?}");
        assert!(row.contains("1 subagent"), "{row}");
        let (hit, id) = app.subagent_strip_hit.first().expect("hit");
        assert_eq!(hit.x, 0);
        assert_eq!(hit.width, 80, "hit stays full width");
        assert_eq!(id, "task-1");
    }

    #[test]
    fn upsert_updates_same_id() {
        let mut app = TuiApp::new(TuiAppConfig::default());
        app.upsert_subagent(crate::app::SubagentUpdate {
            id: "w-0".into(),
            kind: "general".into(),
            description: "do it".into(),
            status: "running".into(),
            activity: "Thinking".into(),
            elapsed_ms: 0,
            output: String::new(),
        });
        app.upsert_subagent(crate::app::SubagentUpdate {
            id: "w-0".into(),
            kind: "general".into(),
            description: "do it".into(),
            status: "completed".into(),
            activity: String::new(),
            elapsed_ms: 1400,
            output: "done".into(),
        });
        assert_eq!(app.subagents.len(), 1);
        assert!(!app.subagents[0].is_running());
        assert_eq!(app.subagents[0].output, "done");
        assert_eq!(app.running_subagent_count(), 0);
    }
}
