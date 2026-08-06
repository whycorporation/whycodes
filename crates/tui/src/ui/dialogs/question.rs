//! Interactive questionnaire modal (Grok-style ask_user_question).
//!
//! Layout:
//! ```text
//! ─ Question  (1/2) ─
//!
//! What approach should we take?
//!
//!   ▸ SQLite
//!       Simple local store for v1
//!     Postgres
//!       Better multi-user later
//!     Other…
//!       Type your own answer
//!
//!   > free text when Other or free-form
//!
//! ↑/↓  Space  Enter  Esc
//! ```

use crate::app::QuestionDialogState;
use crate::theme::ThemePalette;
use ratatui::{
    Frame,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
};

use super::base::{DialogChrome, dialog_frame};

pub fn render_question_dialog(
    frame: &mut Frame,
    state: &QuestionDialogState,
    palette: &ThemePalette,
    mouse_pos: Option<(u16, u16)>,
) -> DialogChrome {
    let total_q = state.questions.len().max(1);
    let cur = state.index + 1;
    let title = if total_q > 1 {
        format!(" Question  ({cur}/{total_q})")
    } else {
        "Question".to_string()
    };

    let shortcuts: &[&str] = if state.free_text_focus {
        &["Type answer", "Enter submit", "Esc back", "[✗]"]
    } else if state.current().map(|q| q.multi_select).unwrap_or(false) {
        &["↑/↓", "Space toggle", "Enter done", "Esc cancel"]
    } else {
        &["↑/↓", "Enter select", "o Other", "Esc cancel"]
    };

    let chrome = dialog_frame(
        frame,
        &title,
        shortcuts,
        palette,
        72,
        62,
        mouse_pos,
    );
    let area = chrome.content;
    if area.width == 0 || area.height == 0 {
        return chrome;
    }

    let Some(q) = state.current() else {
        return chrome;
    };

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));
    // Prompt
    for para in q.prompt.split('\n') {
        lines.push(Line::from(Span::styled(
            para.to_string(),
            Style::default()
                .fg(palette.fg)
                .add_modifier(Modifier::BOLD),
        )));
    }
    lines.push(Line::from(""));

    let n_opts = state.option_count();
    for i in 0..n_opts {
        let current = i == state.cursor;
        let is_other = state.is_other_index(i);
        let checked = if q.multi_select && !is_other {
            state.multi_selected.contains(&i)
        } else {
            false
        };

        let (label, description) = if is_other {
            ("Other…".to_string(), "Type your own answer".to_string())
        } else {
            let opt = &q.options[i];
            (opt.label.clone(), opt.description.clone())
        };

        let marker = if q.multi_select && !is_other {
            if checked {
                "[×] "
            } else {
                "[ ] "
            }
        } else if current {
            "▸ "
        } else {
            "  "
        };

        let label_style = if current {
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette.fg)
        };

        lines.push(Line::from(vec![
            Span::styled(
                format!("  {marker}"),
                Style::default().fg(palette.accent),
            ),
            Span::styled(label, label_style),
        ]));

        if !description.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("      {description}"),
                Style::default().fg(palette.dim),
            )));
        }

        // Preview only when focused
        if current
            && !is_other
            && let Some(opt) = q.options.get(i)
            && let Some(ref prev) = opt.preview
            && !prev.is_empty()
        {
            for pl in prev.lines().take(3) {
                lines.push(Line::from(Span::styled(
                    format!("      {pl}"),
                    Style::default().fg(palette.dim).add_modifier(Modifier::ITALIC),
                )));
            }
        }
    }

    // Free-text field
    let show_input = state.free_text_focus
        || state.is_other_index(state.cursor)
        || q.options.is_empty();
    if show_input {
        lines.push(Line::from(""));
        let caret = if state.free_text_focus { "▌" } else { " " };
        let body = if state.free_text.is_empty() && !state.free_text_focus {
            "(type free-text answer)".to_string()
        } else {
            state.free_text.clone()
        };
        let style = if state.free_text_focus {
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette.dim)
        };
        lines.push(Line::from(vec![
            Span::styled("  > ".to_string(), Style::default().fg(palette.dim)),
            Span::styled(body, style),
            Span::styled(caret.to_string(), Style::default().fg(palette.accent)),
        ]));
    }

    if q.multi_select {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Multi-select: Space to toggle, Enter when done.".to_string(),
            Style::default().fg(palette.dim),
        )));
    }

    let max_rows = area.height as usize;
    if lines.len() > max_rows {
        lines.truncate(max_rows.saturating_sub(1));
        lines.push(Line::from(Span::styled(
            "  …".to_string(),
            Style::default().fg(palette.dim),
        )));
    }

    let body = Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: false })
        .style(Style::default().bg(palette.bg));
    frame.render_widget(body, area);
    chrome
}
