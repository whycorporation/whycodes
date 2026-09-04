//! Interactive questionnaire panel (Grok-style ask_user_question).
//!
//! Docked to the **bottom** of the screen, options as one selectable row each:
//! ```text
//! ─ Question  (1/2) ─
//!
//! What approach should we take?
//!
//!   ▸ 1. SQLite — Simple local store for v1
//!     2. Postgres — Better multi-user later
//!     3. Other… — Type your own answer
//!
//!   > free text when Other or free-form
//!
//! ↑/↓  ←/→ q  y copy  Enter  Esc
//! ```

use crate::app::QuestionDialogState;
use crate::theme::ThemePalette;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
};

use super::base::{DialogChrome, DialogPlacement, dialog_frame_placed};

/// Paint result with optional option-list hit area (one row per option).
pub struct QuestionPaint {
    pub chrome: DialogChrome,
    /// Content rect covering option rows only (for mouse click → index).
    pub list_area: Option<Rect>,
    pub list_total: usize,
}

pub fn render_question_dialog(
    frame: &mut Frame,
    state: &QuestionDialogState,
    palette: &ThemePalette,
    mouse_pos: Option<(u16, u16)>,
) -> QuestionPaint {
    let total_q = state.questions.len().max(1);
    let cur = state.index + 1;
    let title = if total_q > 1 {
        format!("Question  ({cur}/{total_q})")
    } else {
        "Question".to_string()
    };

    let shortcuts: &[&str] = if state.free_text_focus {
        &["Type answer", "Enter submit", "Esc back", "y copy", "[✗]"]
    } else if total_q > 1 {
        if state.current().map(|q| q.multi_select).unwrap_or(false) {
            &["↑/↓", "Space", "←/→ nav", "y copy", "Enter", "Esc"]
        } else {
            &["↑/↓", "←/→ nav", "y copy", "Enter", "o Other", "Esc"]
        }
    } else if state.current().map(|q| q.multi_select).unwrap_or(false) {
        &["↑/↓", "Space toggle", "y copy", "Enter done", "Esc"]
    } else {
        &["↑/↓", "y copy", "Enter select", "o Other", "Esc"]
    };

    // Height scales with option count; dock bottom so it sits above the prompt.
    let n_opts = state.option_count().max(1);
    let prompt_lines = state
        .current()
        .map(|q| q.prompt.lines().count().max(1))
        .unwrap_or(1);
    // title chrome + prompt + blank + options + free-text + multi hint + pad
    let content_rows = 2 + prompt_lines + 1 + n_opts + 3 + 2;
    let area_h = frame.area().height.max(1);
    // Phone / keyboard-shrunk heights: keep a usable share of the viewport
    // instead of a 22% sliver that clips options.
    let percent_y = ((content_rows as u16 * 100) / area_h).clamp(40, 90);

    let chrome = dialog_frame_placed(
        frame,
        &title,
        shortcuts,
        palette,
        88,
        percent_y,
        mouse_pos,
        DialogPlacement::Bottom,
    );
    let area = chrome.content;
    if area.width == 0 || area.height == 0 {
        return QuestionPaint {
            chrome,
            list_area: None,
            list_total: 0,
        };
    }

    let Some(q) = state.current() else {
        return QuestionPaint {
            chrome,
            list_area: None,
            list_total: 0,
        };
    };

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));
    // Prompt
    for para in q.prompt.split('\n') {
        lines.push(Line::from(Span::styled(
            para.to_string(),
            Style::default().fg(palette.fg).add_modifier(Modifier::BOLD),
        )));
    }
    lines.push(Line::from(""));

    let list_start_row = lines.len();
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
            if checked { "[×] " } else { "[ ] " }
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

        // One selectable row: "N. Label — description"
        let num = i + 1;
        let mut main = format!("{num}. {label}");
        if !description.is_empty() {
            main.push_str(" — ");
            main.push_str(&description);
        }

        lines.push(Line::from(vec![
            Span::styled(format!("  {marker}"), Style::default().fg(palette.accent)),
            Span::styled(main, label_style),
        ]));
    }
    let list_end_row = lines.len();

    // Preview under the focused option (not part of hit list)
    if !state.free_text_focus
        && !state.is_other_index(state.cursor)
        && let Some(opt) = q.options.get(state.cursor)
        && let Some(ref prev) = opt.preview
        && !prev.is_empty()
    {
        for pl in prev.lines().take(3) {
            lines.push(Line::from(Span::styled(
                format!("      {pl}"),
                Style::default()
                    .fg(palette.dim)
                    .add_modifier(Modifier::ITALIC),
            )));
        }
    }

    // Free-text field
    let show_input =
        state.free_text_focus || state.is_other_index(state.cursor) || q.options.is_empty();
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

    if total_q > 1 && !state.free_text_focus {
        lines.push(Line::from(Span::styled(
            "  ←/→ or [/] navigate questions · y copy".to_string(),
            Style::default().fg(palette.dim),
        )));
    }

    // Map option list rows to screen Y for mouse hit-testing (1 row = 1 option).
    let list_total = n_opts;
    let list_area = if list_total > 0 && list_end_row > list_start_row {
        let max_rows = area.height as usize;
        // Only rows that fit in the painted area are clickable.
        let visible = (list_end_row - list_start_row).min(max_rows.saturating_sub(list_start_row));
        if visible > 0 && list_start_row < max_rows {
            Some(Rect {
                x: area.x,
                y: area.y + list_start_row as u16,
                width: area.width,
                height: visible as u16,
            })
        } else {
            None
        }
    } else {
        None
    };

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

    QuestionPaint {
        chrome,
        list_area,
        list_total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeName;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use whycodes_tools::question::{QuestionOption, QuestionSpec};

    #[test]
    fn render_question_paints_prompt_and_other() {
        let palette = ThemeName::DefaultDark.palette();
        let state = QuestionDialogState::new(vec![QuestionSpec {
            prompt: "Pick a store?".into(),
            options: vec![QuestionOption {
                label: "SQLite".into(),
                description: "local".into(),
                preview: None,
            }],
            multi_select: false,
        }]);
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                let paint = render_question_dialog(f, &state, &palette, None);
                assert!(paint.list_total >= 1);
            })
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(text.contains("Pick a store"), "{text}");
        assert!(text.contains("SQLite") || text.contains("Other"), "{text}");
    }
}
