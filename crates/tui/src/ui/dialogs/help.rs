// ── ui/dialogs/help.rs: Keyboard Shortcuts cheatsheet ─────────────────
// Visual model: Grok Build `shortcuts_help` (searchable cheatsheet).
//   ModalWindow chrome · " / to search" · full-width divider ·
//   `── Section ──` headers · `◆ key` + right-aligned description.

use crate::app::TuiApp;
use crate::theme::ThemePalette;
use crate::ui::scrollbar::{ScrollbarColors, paint_scrollbar};
use ratatui::{Frame, layout::Rect, style::Style};

use super::base::{dialog_frame, paint_divider};
use super::select::{paint_picker_row, paint_section_header};

enum HelpLine {
    Header(&'static str),
    Binding {
        key: &'static str,
        desc: &'static str,
    },
}

fn catalog() -> Vec<HelpLine> {
    vec![
        HelpLine::Header("General"),
        HelpLine::Binding {
            key: "/help",
            desc: "Open this help",
        },
        HelpLine::Binding {
            key: "Tab",
            desc: "Focus prompt ↔ scrollback",
        },
        HelpLine::Binding {
            key: "Ctrl+T",
            desc: "Cycle primary agent",
        },
        HelpLine::Binding {
            key: ":",
            desc: "Enter command mode",
        },
        HelpLine::Binding {
            key: "Esc",
            desc: "Cancel turn · double-Esc clear draft",
        },
        HelpLine::Binding {
            key: "Ctrl+C",
            desc: "Clear draft / quit",
        },
        HelpLine::Header("Sessions"),
        HelpLine::Binding {
            key: "Ctrl+O",
            desc: "Live session dashboard",
        },
        HelpLine::Binding {
            key: "Ctrl+N",
            desc: "New session",
        },
        HelpLine::Binding {
            key: "Ctrl+Tab",
            desc: "Switch session (recent)",
        },
        HelpLine::Binding {
            key: "Ctrl+PgUp/PgDn",
            desc: "Cycle sessions",
        },
        HelpLine::Binding {
            key: "Ctrl+W",
            desc: "Close live session (in /sessions)",
        },
        HelpLine::Header("Scrollback"),
        HelpLine::Binding {
            key: "j/k · ↑/↓",
            desc: "Select message",
        },
        HelpLine::Binding {
            key: "Ctrl+↑/↓",
            desc: "Scroll transcript",
        },
        HelpLine::Binding {
            key: "PgUp/PgDn",
            desc: "Page scroll",
        },
        HelpLine::Binding {
            key: "g / G",
            desc: "Top / bottom",
        },
        HelpLine::Binding {
            key: "Shift+←/→",
            desc: "Prev / next user turn",
        },
        HelpLine::Binding {
            key: "y",
            desc: "Copy selected message",
        },
        HelpLine::Binding {
            key: "e / h",
            desc: "Toggle thinking fold",
        },
        HelpLine::Binding {
            key: "l",
            desc: "Toggle tool results",
        },
        HelpLine::Binding {
            key: "Space / i",
            desc: "Focus prompt",
        },
        HelpLine::Header("Prompt"),
        HelpLine::Binding {
            key: "Enter",
            desc: "Send message",
        },
        HelpLine::Binding {
            key: "@",
            desc: "Mention a file (fuzzy picker)",
        },
        HelpLine::Binding {
            key: "Ctrl+Space",
            desc: "File picker (same as @)",
        },
        HelpLine::Binding {
            key: "Ctrl+U",
            desc: "Clear input",
        },
        HelpLine::Binding {
            key: "Ctrl+W / Ctrl+⌫",
            desc: "Delete previous word",
        },
        HelpLine::Binding {
            key: "Ctrl+Del / Alt+D",
            desc: "Delete next word",
        },
        HelpLine::Binding {
            key: "Ctrl+←/→",
            desc: "Move by word",
        },
        HelpLine::Binding {
            key: "↑/↓",
            desc: "History (empty prompt)",
        },
        HelpLine::Header("Dialogs & commands"),
        HelpLine::Binding {
            key: "Ctrl+P",
            desc: "Provider setup",
        },
        HelpLine::Binding {
            key: "Ctrl+M",
            desc: "Model selection",
        },
        HelpLine::Binding {
            key: "Ctrl+B",
            desc: "Toggle sidebar",
        },
        HelpLine::Binding {
            key: "Ctrl+. / Ctrl+,",
            desc: "Sidebar next / prev tab",
        },
        HelpLine::Binding {
            key: "1–6",
            desc: "Sidebar tab (scrollback)",
        },
        HelpLine::Binding {
            key: "[ / ]",
            desc: "Sidebar tabs (scrollback)",
        },
        HelpLine::Binding {
            key: "Ctrl+G",
            desc: "Toggle tasks panel",
        },
        HelpLine::Binding {
            key: "Ctrl+A",
            desc: "Toggle auto-scroll",
        },
        HelpLine::Binding {
            key: "Ctrl+L",
            desc: "Clear session",
        },
        HelpLine::Binding {
            key: "/help",
            desc: "This screen",
        },
        HelpLine::Binding {
            key: "/connect",
            desc: "Provider help",
        },
        HelpLine::Binding {
            key: "/login",
            desc: "Sign in (OAuth)",
        },
        HelpLine::Binding {
            key: "/resume",
            desc: "Session history",
        },
        HelpLine::Binding {
            key: ":theme",
            desc: "Change theme",
        },
        HelpLine::Binding {
            key: ":q",
            desc: "Quit",
        },
    ]
}

fn matches_query(line: &HelpLine, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_ascii_lowercase();
    match line {
        HelpLine::Header(label) => label.to_ascii_lowercase().contains(&q),
        HelpLine::Binding { key, desc } => {
            key.to_ascii_lowercase().contains(&q) || desc.to_ascii_lowercase().contains(&q)
        }
    }
}

fn visible_lines(query: &str) -> Vec<HelpLine> {
    let all = catalog();
    if query.is_empty() {
        return all;
    }
    let mut out = Vec::new();
    let mut pending_header: Option<HelpLine> = None;
    for line in all {
        match &line {
            HelpLine::Header(_) => {
                if matches_query(&line, query) {
                    out.push(line);
                    pending_header = None;
                } else {
                    pending_header = Some(line);
                }
            }
            HelpLine::Binding { .. } => {
                if matches_query(&line, query) {
                    if let Some(h) = pending_header.take() {
                        out.push(h);
                    }
                    out.push(line);
                }
            }
        }
    }
    out
}

/// Paint help and register the same modal hit boxes as list/confirm dialogs.
pub fn render_help_overlay(frame: &mut Frame, app: &mut TuiApp, palette: &ThemePalette) {
    let chrome = dialog_frame(
        frame,
        "Keyboard Shortcuts",
        &["↑/↓ nav", "/ search", "Esc close"],
        palette,
        app.mouse_pos,
    );
    let area = chrome.content;
    if area.width == 0 || area.height == 0 {
        app.apply_modal_chrome(chrome.close_hit, chrome.modal, None);
        return;
    }

    let searching = app.help_searching || !app.help_query.is_empty();
    paint_search_bar(frame, area, &app.help_query, searching, palette);
    let sep_y = area.y + 1;
    if sep_y < area.y + area.height {
        paint_divider(frame, chrome.inner_x, sep_y, chrome.inner_width, palette);
    }

    let entries_y = sep_y.saturating_add(1);
    let entries_h = area.y.saturating_add(area.height).saturating_sub(entries_y);
    let entries = Rect {
        x: area.x,
        y: entries_y,
        width: area.width,
        height: entries_h,
    };
    if entries.height == 0 {
        app.apply_modal_chrome(chrome.close_hit, chrome.modal, None);
        return;
    }

    let lines = visible_lines(&app.help_query);
    let total = lines.len();
    let max_rows = entries.height as usize;
    let max_scroll = total.saturating_sub(max_rows);
    let start = app.help_scroll.min(max_scroll);
    if app.help_scroll > max_scroll {
        app.help_scroll = max_scroll;
    }
    let needs_scrollbar = total > max_rows;
    let list_width = if needs_scrollbar {
        entries.width.saturating_sub(1)
    } else {
        entries.width
    };
    let list_area = Rect {
        x: entries.x,
        y: entries.y,
        width: list_width,
        height: entries.height,
    };

    if lines.is_empty() {
        paint_picker_row(
            frame.buffer_mut(),
            Rect {
                x: list_area.x,
                y: list_area.y,
                width: list_area.width,
                height: 1,
            },
            "No matches",
            None,
            false,
            palette,
            true,
        );
    } else {
        for (row_i, line) in lines.iter().skip(start).take(max_rows).enumerate() {
            let row = Rect {
                x: list_area.x,
                y: list_area.y + row_i as u16,
                width: list_area.width,
                height: 1,
            };
            match line {
                HelpLine::Header(label) => {
                    paint_section_header(frame.buffer_mut(), row, label, palette);
                }
                HelpLine::Binding { key, desc } => {
                    paint_picker_row(
                        frame.buffer_mut(),
                        row,
                        key,
                        Some(desc),
                        false,
                        palette,
                        false,
                    );
                }
            }
        }
    }

    let scrollbar_hit = if needs_scrollbar {
        let colors = ScrollbarColors::from_palette(palette);
        let sb = Rect {
            x: entries.x + entries.width.saturating_sub(1),
            y: entries.y,
            width: 1,
            height: entries.height,
        };
        paint_scrollbar(
            frame.buffer_mut(),
            sb,
            total,
            max_rows,
            start,
            colors.track,
            colors.thumb,
        );
        Some(sb)
    } else {
        None
    };

    app.apply_modal_chrome(chrome.close_hit, chrome.modal, scrollbar_hit);
    app.dialog_list_hit = Some(list_area);
    app.dialog_list_scroll_start = start;
    app.dialog_list_visible = max_rows;
    app.dialog_list_total = total;
}

fn paint_search_bar(
    frame: &mut Frame,
    area: Rect,
    query: &str,
    searching: bool,
    palette: &ThemePalette,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let row = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    let style = Style::default().fg(palette.dim).bg(palette.bg);
    let buf = frame.buffer_mut();
    for x in row.x..row.x.saturating_add(row.width) {
        if let Some(cell) = buf.cell_mut((x, row.y)) {
            cell.set_symbol(" ");
            cell.set_style(style);
        }
    }
    if searching || !query.is_empty() {
        let text = format!(" search: {query}");
        buf.set_stringn(row.x, row.y, &text, row.width as usize, style);
        let cursor_col = (text.chars().count() as u16).min(row.width.saturating_sub(1));
        if let Some(cell) = buf.cell_mut((row.x + cursor_col, row.y)) {
            cell.set_style(Style::default().fg(palette.bg).bg(palette.fg));
        }
    } else {
        buf.set_stringn(row.x, row.y, " / to search", row.width as usize, style);
    }
}
