// ── ui/dialogs/provider.rs: Provider dialog ────────────────────────────
// Two modes: Select from list, or Add Custom.

use crate::app::{AuthMethod, ProviderDialogMode, TuiApp};
use crate::theme::ThemePalette;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
};

use super::base::dialog_frame;

pub fn render_provider_dialog(frame: &mut Frame, app: &TuiApp, palette: &ThemePalette) {
    let pd = &app.provider_dialog;

    match pd.mode {
        ProviderDialogMode::Select => render_provider_select(frame, app, palette),
        ProviderDialogMode::AddCustom => render_provider_add(frame, app, palette),
    }
}

fn render_provider_select(frame: &mut Frame, app: &TuiApp, palette: &ThemePalette) {
    let area = dialog_frame(frame, " Select Provider ", palette, 60, 70);
    let pd = &app.provider_dialog;

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        " Choose a provider or add a custom one: ",
        Style::default().fg(palette.fg),
    )));
    lines.push(Line::from(""));

    for (i, name) in pd.providers.iter().enumerate() {
        let prefix = if i == pd.selected { "▸ " } else { "  " };
        let style = if i == pd.selected {
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette.fg)
        };
        lines.push(Line::from(Span::styled(
            format!("{}{}", prefix, name),
            style,
        )));
    }

    // Add custom entry.
    let idx = pd.providers.len();
    let prefix = if idx == pd.selected { "▸ " } else { "  " };
    let style = if idx == pd.selected {
        Style::default()
            .fg(palette.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(palette.dim)
    };
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("{}+ Add Custom Provider...", prefix),
        style,
    )));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Enter: select | a: add custom | Esc: close ",
        Style::default().fg(palette.dim),
    )));

    let text = Text::from(lines);
    let p = Paragraph::new(text).wrap(Wrap { trim: true });
    frame.render_widget(p, area);
}

fn render_provider_add(frame: &mut Frame, app: &TuiApp, palette: &ThemePalette) {
    let area = dialog_frame(frame, " Add Custom Provider ", palette, 70, 65);
    let pd = &app.provider_dialog;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(3)])
        .split(area);

    let body = chunks[0];
    let footer = chunks[1];

    let mut lines: Vec<Line> = Vec::new();

    // Form fields.
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
            format!("{}{: <10} {}", prefix, label, display_val),
            style,
        )));

        if active {
            lines.push(Line::from(Span::styled(
                format!("              {}", hint),
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
        format!("  Auth Method: {}", auth_label),
        Style::default().fg(palette.fg),
    )));

    // Error.
    if let Some(ref err) = pd.error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(" ✗ {}", err),
            Style::default().fg(palette.error),
        )));
    }

    // Saved.
    if pd.saved {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " ✓ Provider saved!",
            Style::default().fg(palette.success),
        )));
    }

    let text = Text::from(lines);
    let p = Paragraph::new(text).wrap(Wrap { trim: true });
    frame.render_widget(p, body);

    // Footer.
    let footer_text = Span::styled(
        " Ctrl+S: Save | Esc/Ctrl+C: Cancel | Tab: Next Field | Enter: Save ",
        Style::default().fg(palette.dim),
    );
    let fp = Paragraph::new(Text::from(Line::from(footer_text)));
    frame.render_widget(fp, footer);
}

pub fn render_model_dialog(frame: &mut Frame, app: &TuiApp, palette: &ThemePalette) {
    let area = dialog_frame(frame, " Select Model ", palette, 50, 50);
    let ms = &app.model_selection;

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        " Choose a model: ",
        Style::default().fg(palette.fg),
    )));
    lines.push(Line::from(""));

    if ms.models.is_empty() {
        lines.push(Line::from(Span::styled(
            " No models configured. Add a provider first.",
            Style::default().fg(palette.dim),
        )));
    } else {
        for (i, (provider, model)) in ms.models.iter().enumerate() {
            let prefix = if i == ms.selected { "▸ " } else { "  " };
            let style = if i == ms.selected {
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(palette.fg)
            };
            lines.push(Line::from(Span::styled(
                format!("{}{} / {}", prefix, provider, model),
                style,
            )));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Enter: select | Esc: close ",
        Style::default().fg(palette.dim),
    )));

    let text = Text::from(lines);
    let p = Paragraph::new(text).wrap(Wrap { trim: true });
    frame.render_widget(p, area);
}
