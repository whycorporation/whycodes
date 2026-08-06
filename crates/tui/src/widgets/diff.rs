// ── widgets/diff.rs: Diff viewer widget ────────────────────────────────
// Renders unified or side-by-side diffs with syntax highlighting.

use crate::theme::ThemePalette;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
};

/// Render a unified diff.
pub fn render_unified_diff(
    frame: &mut Frame,
    area: Rect,
    diff_lines: &[DiffLine],
    palette: &ThemePalette,
) {
    let mut lines: Vec<Line> = Vec::new();

    let add_bg = palette.callout_bg(palette.diff_add);
    let rem_bg = palette.callout_bg(palette.diff_remove);

    for dl in diff_lines {
        let (prefix, color, bg) = match dl.kind {
            DiffLineKind::Add => ("+", palette.diff_add, Some(add_bg)),
            DiffLineKind::Remove => ("-", palette.diff_remove, Some(rem_bg)),
            DiffLineKind::Context => (" ", palette.fg, None),
            DiffLineKind::Header => ("@", palette.diff_hunk, None),
        };

        let style = match bg {
            Some(b) => Style::default().fg(color).bg(b),
            None => Style::default().fg(color),
        };

        lines.push(Line::from(vec![
            Span::styled(prefix, style.add_modifier(Modifier::BOLD)),
            Span::styled(dl.content.clone(), style),
        ]));
    }

    let p = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(palette.border))
                .title(" Diff "),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(p, area);
}

/// Render a side-by-side diff.
pub fn render_split_diff(
    frame: &mut Frame,
    area: Rect,
    old_lines: &[DiffLine],
    new_lines: &[DiffLine],
    palette: &ThemePalette,
) {
    let [left_area, right_area] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .areas(area);

    // Left (old).
    let mut left_text: Vec<Line> = Vec::new();
    for dl in old_lines {
        let color = match dl.kind {
            DiffLineKind::Remove => palette.diff_remove,
            _ => palette.fg,
        };
        left_text.push(Line::from(Span::styled(
            dl.content.clone(),
            Style::default().fg(color),
        )));
    }
    let left_p = Paragraph::new(Text::from(left_text))
        .block(Block::default().borders(Borders::ALL).title(" Old "));
    frame.render_widget(left_p, left_area);

    // Right (new).
    let mut right_text: Vec<Line> = Vec::new();
    for dl in new_lines {
        let color = match dl.kind {
            DiffLineKind::Add => palette.diff_add,
            _ => palette.fg,
        };
        right_text.push(Line::from(Span::styled(
            dl.content.clone(),
            Style::default().fg(color),
        )));
    }
    let right_p = Paragraph::new(Text::from(right_text))
        .block(Block::default().borders(Borders::ALL).title(" New "));
    frame.render_widget(right_p, right_area);
}

/// A single diff line.
#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Add,
    Remove,
    Context,
    Header,
}

/// Parse a unified diff string into DiffLine structs.
pub fn parse_unified_diff(diff_text: &str) -> Vec<DiffLine> {
    let mut lines = Vec::new();
    for raw in diff_text.lines() {
        if raw.is_empty() {
            continue;
        }
        let (kind, content) =
            if raw.starts_with("+++") || raw.starts_with("---") || raw.starts_with("@@") {
                (DiffLineKind::Header, raw.to_string())
            } else if let Some(rest) = raw.strip_prefix('+') {
                (DiffLineKind::Add, rest.to_string())
            } else if let Some(rest) = raw.strip_prefix('-') {
                (DiffLineKind::Remove, rest.to_string())
            } else {
                (DiffLineKind::Context, raw.to_string())
            };
        lines.push(DiffLine { kind, content });
    }
    lines
}
