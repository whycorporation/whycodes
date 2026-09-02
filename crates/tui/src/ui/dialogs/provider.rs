// ── ui/dialogs/provider.rs: Provider dialog ────────────────────────────
// Two modes: Select from list, or Add Custom.
// Chrome matches Grok ModalWindow via dialog_frame.
// List modes paint a scrollbar when the item list overflows the content area.

use crate::app::{AuthMethod, ProviderDialogMode, TuiApp};
use crate::theme::ThemePalette;
use crate::ui::scrollbar::{ScrollbarColors, paint_scrollbar, scroll_to_selected};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
};

use super::base::{dialog_frame, paint_divider};
use super::select::{paint_group_header, paint_picker_row, paint_search_bar};
use crate::app::ModelPickerRow;

pub fn render_provider_dialog(frame: &mut Frame, app: &mut TuiApp, palette: &ThemePalette) {
    match app.provider_dialog.mode {
        ProviderDialogMode::Select => render_provider_select(frame, app, palette),
        ProviderDialogMode::AddCustom => render_provider_add(frame, app, palette),
    }
}

fn render_provider_select(frame: &mut Frame, app: &mut TuiApp, palette: &ThemePalette) {
    let chrome = dialog_frame(
        frame,
        "Select Provider",
        &["↑/↓ nav", "Enter select", "Esc close"],
        palette,
        app.mouse_pos,
    );
    let area = chrome.content;
    if area.width == 0 || area.height == 0 {
        app.apply_modal_chrome(chrome.close_hit, chrome.modal, None);
        return;
    }
    let pd = &app.provider_dialog;

    // Header (2 lines) is fixed; the selectable rows scroll below.
    const HEADER_ROWS: usize = 2;
    // providers + "Add Custom"
    let item_count = pd.providers.len() + 1;
    let list_budget = (area.height as usize).saturating_sub(HEADER_ROWS).max(1);
    let needs_scrollbar = item_count > list_budget;
    let list_width = if needs_scrollbar {
        area.width.saturating_sub(1)
    } else {
        area.width
    };
    let start = scroll_to_selected(pd.selected, item_count, list_budget);

    let lines = vec![
        Line::from(Span::styled(
            "Choose a provider or add a custom one:",
            Style::default().fg(palette.dim),
        )),
        Line::from(""),
    ];

    // Hint + blank occupy HEADER_ROWS; selectable rows start below.
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().bg(palette.bg)),
        Rect {
            x: area.x,
            y: area.y,
            width: list_width,
            height: HEADER_ROWS as u16,
        },
    );

    let rows_area = Rect {
        x: area.x,
        y: area.y + HEADER_ROWS as u16,
        width: list_width,
        height: area.height.saturating_sub(HEADER_ROWS as u16),
    };
    for (row_i, i) in (start..start.saturating_add(list_budget).min(item_count)).enumerate() {
        let row = Rect {
            x: rows_area.x,
            y: rows_area.y + row_i as u16,
            width: rows_area.width,
            height: 1,
        };
        if i < pd.providers.len() {
            paint_picker_row(
                frame.buffer_mut(),
                row,
                &pd.providers[i],
                None,
                i == pd.selected,
                palette,
                false,
            );
        } else {
            paint_picker_row(
                frame.buffer_mut(),
                row,
                "+ Add Custom Provider…",
                None,
                i == pd.selected,
                palette,
                true,
            );
        }
    }

    let mut scrollbar_hit = None;
    if needs_scrollbar {
        let colors = ScrollbarColors::from_palette(palette);
        // Bar covers the list body only (below header).
        let bar_h = area.height.saturating_sub(HEADER_ROWS as u16);
        if bar_h > 0 {
            let sb = Rect {
                x: area.x + area.width.saturating_sub(1),
                y: area.y + HEADER_ROWS as u16,
                width: 1,
                height: bar_h,
            };
            paint_scrollbar(
                frame.buffer_mut(),
                sb,
                item_count,
                list_budget,
                start,
                colors.track,
                colors.thumb,
            );
            scrollbar_hit = Some(sb);
        }
    }

    app.apply_select_paint(
        chrome.close_hit,
        Some(rows_area),
        scrollbar_hit,
        start,
        list_budget,
        item_count,
        Some(chrome.modal),
    );
}

fn render_provider_add(frame: &mut Frame, app: &mut TuiApp, palette: &ThemePalette) {
    let chrome = dialog_frame(
        frame,
        "Add Custom Provider",
        &["Ctrl+S save", "Tab next", "Esc close"],
        palette,
        app.mouse_pos,
    );
    app.apply_modal_chrome(chrome.close_hit, chrome.modal, None);
    let area = chrome.content;
    let pd = &app.provider_dialog;

    let mut lines: Vec<Line> = Vec::new();

    let form_fields = [
        ("Name", &pd.form_name, "(e.g., groq, together)"),
        ("API Key", &pd.form_api_key, "(leave empty if not required)"),
        (
            "Base URL",
            &pd.form_base_url,
            "(e.g., https://api.groq.com/openai/v1)",
        ),
        (
            "Headers",
            &pd.form_headers,
            "(optional: key1=val1,key2=val2)",
        ),
    ];

    for (i, (label, value, hint)) in form_fields.iter().enumerate() {
        let active = i == pd.active_field;
        let style = if active {
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette.fg)
        };
        let prefix = if active { "▸ " } else { "  " };

        let display_val = if *label == "API Key" && !value.is_empty() {
            "*".repeat(value.len().min(20))
        } else {
            value.to_string()
        };

        lines.push(Line::from(Span::styled(
            format!("{prefix}{label:<10} {display_val}"),
            style,
        )));

        if active {
            lines.push(Line::from(Span::styled(
                format!("              {hint}"),
                Style::default().fg(palette.dim),
            )));
        }
    }

    // Auth method selector.
    lines.push(Line::from(""));
    let auth_label = match pd.form_auth_method {
        AuthMethod::ApiKey => "API Key",
        AuthMethod::Bearer => "Bearer Token",
        AuthMethod::Basic => "Basic Auth",
        AuthMethod::None => "None",
    };
    lines.push(Line::from(Span::styled(
        format!("  Auth Method: {auth_label}"),
        Style::default().fg(palette.fg),
    )));

    if let Some(ref err) = pd.error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(" ✗ {err}"),
            Style::default().fg(palette.error),
        )));
    }

    if pd.saved {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " ✓ Provider saved!",
            Style::default().fg(palette.success),
        )));
    }

    let p = Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: true })
        .style(Style::default().bg(palette.bg));
    frame.render_widget(p, area);
}

pub fn render_model_dialog(frame: &mut Frame, app: &mut TuiApp, palette: &ThemePalette) {
    let chrome = dialog_frame(
        frame,
        "Select Model",
        &[
            "↑/↓ nav",
            "/ search",
            "←/→ fold",
            "Enter select",
            "Esc close",
        ],
        palette,
        app.mouse_pos,
    );
    let area = chrome.content;
    if area.width == 0 || area.height == 0 {
        app.apply_modal_chrome(chrome.close_hit, chrome.modal, None);
        return;
    }

    app.model_selection.clamp_selected();
    let rows = app.model_selection.visible_rows();
    let selected = app.model_selection.selected;
    let searching = app.model_selection.searching;
    let query = app.model_selection.query.clone();
    let empty_catalog = app.model_selection.models.is_empty();

    const HEADER_ROWS: usize = 2;
    let item_count = rows.len();
    let list_budget = (area.height as usize).saturating_sub(HEADER_ROWS).max(1);
    let needs_scrollbar = item_count > list_budget;
    let list_width = if needs_scrollbar {
        area.width.saturating_sub(1)
    } else {
        area.width
    };
    let start = scroll_to_selected(selected, item_count, list_budget);

    let search_area = Rect {
        x: area.x,
        y: area.y,
        width: list_width,
        height: 1,
    };
    paint_search_bar(frame, search_area, &query, searching, palette);
    paint_divider(frame, area.x, area.y.saturating_add(1), list_width, palette);

    let rows_area = Rect {
        x: area.x,
        y: area.y + HEADER_ROWS as u16,
        width: list_width,
        height: area.height.saturating_sub(HEADER_ROWS as u16),
    };
    if empty_catalog {
        paint_picker_row(
            frame.buffer_mut(),
            Rect {
                x: rows_area.x,
                y: rows_area.y,
                width: rows_area.width,
                height: 1,
            },
            "No models configured. Add a provider first.",
            None,
            false,
            palette,
            true,
        );
    } else if rows.is_empty() {
        paint_picker_row(
            frame.buffer_mut(),
            Rect {
                x: rows_area.x,
                y: rows_area.y,
                width: rows_area.width,
                height: 1,
            },
            "No matching models.",
            None,
            false,
            palette,
            true,
        );
    } else {
        for (row_i, (i, row)) in rows
            .iter()
            .enumerate()
            .skip(start)
            .take(list_budget)
            .enumerate()
        {
            let cell = Rect {
                x: rows_area.x,
                y: rows_area.y + row_i as u16,
                width: rows_area.width,
                height: 1,
            };
            match row {
                ModelPickerRow::Header {
                    provider,
                    count,
                    collapsed,
                } => paint_group_header(
                    frame.buffer_mut(),
                    cell,
                    provider,
                    *count,
                    *collapsed,
                    i == selected,
                    palette,
                ),
                ModelPickerRow::Model { model, .. } => paint_picker_row(
                    frame.buffer_mut(),
                    cell,
                    model,
                    None,
                    i == selected,
                    palette,
                    false,
                ),
            }
        }
    }

    let mut scrollbar_hit = None;
    if needs_scrollbar && !rows.is_empty() {
        let colors = ScrollbarColors::from_palette(palette);
        let bar_h = area.height.saturating_sub(HEADER_ROWS as u16);
        if bar_h > 0 {
            let sb = Rect {
                x: area.x + area.width.saturating_sub(1),
                y: area.y + HEADER_ROWS as u16,
                width: 1,
                height: bar_h,
            };
            paint_scrollbar(
                frame.buffer_mut(),
                sb,
                item_count,
                list_budget,
                start,
                colors.track,
                colors.thumb,
            );
            scrollbar_hit = Some(sb);
        }
    }

    app.apply_select_paint(
        chrome.close_hit,
        Some(rows_area),
        scrollbar_hit,
        start,
        list_budget,
        item_count,
        Some(chrome.modal),
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn provider_module_loads() {
        assert!(!module_path!().is_empty());
    }
}
