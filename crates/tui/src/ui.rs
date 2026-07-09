use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::app::{App, Mode};

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Main vertical layout: chat, input bar, status bar
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),       // Chat area
            Constraint::Length(3),     // Input bar
            Constraint::Length(1),     // Status bar
        ])
        .split(area);

    render_chat(frame, chunks[0], app);
    render_input(frame, chunks[1], app);
    render_status(frame, chunks[2], app);

    // Overlay help if in Help mode
    if matches!(app.mode, Mode::Help) {
        render_help(frame, area);
    }
}

fn render_chat(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines: Vec<Line> = Vec::new();

    let msg_count = app.messages.len();
    let scroll_offset = app.scroll;

    // Show scroll indicator if scrolled up
    if scroll_offset > 0 && msg_count > scroll_offset {
        lines.push(Line::from(Span::styled(
            format!("  ↑ {} older messages hidden ↑", scroll_offset),
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(""));
    }

    let visible_start = if msg_count > scroll_offset {
        msg_count - scroll_offset
    } else {
        0
    };

    for (i, (role, text)) in app.messages.iter().enumerate() {
        if i < visible_start {
            continue;
        }

        let role_style = match role.as_str() {
            "user" => Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            "assistant" => Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            "system" => Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            "cmd" => Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
            _ => Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        };

        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", role), role_style),
            Span::raw(text),
        ]));
        lines.push(Line::from(""));
    }

    let text = Text::from(lines);
    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .title(" Chat ")
                .borders(Borders::ALL),
        )
        .wrap(Wrap { trim: true });

    frame.render_widget(paragraph, area);
}

fn render_input(frame: &mut Frame, area: Rect, app: &App) {
    let prompt = match app.mode {
        Mode::Normal => "> ",
        Mode::Command => "", // Command input starts with ':'
        Mode::Help => "[Press Esc/q/? to exit help] ",
    };

    let input_text = format!("{}{}", prompt, app.input);

    // Show cursor position
    let cursor = if app.input.is_empty() {
        Span::styled("█", Style::default().fg(Color::Yellow))
    } else {
        Span::raw("")
    };

    let input_display = match app.mode {
        Mode::Help => Line::from(Span::styled(
            "  Press Esc, q, or ? to return to normal mode",
            Style::default().fg(Color::Yellow),
        )),
        _ => Line::from(vec![
            Span::raw(input_text),
            cursor,
        ]),
    };

    let input_style = match app.mode {
        Mode::Command => Style::default().fg(Color::Magenta),
        Mode::Help => Style::default().fg(Color::Yellow),
        _ => Style::default().fg(Color::White),
    };

    let paragraph = Paragraph::new(Text::from(vec![input_display]))
        .block(
            Block::default()
                .title(" Input ")
                .borders(Borders::ALL)
                .style(input_style),
        );

    frame.render_widget(paragraph, area);
}

fn render_status(frame: &mut Frame, area: Rect, app: &App) {
    let mode_str = match app.mode {
        Mode::Normal => "NORMAL",
        Mode::Help => "HELP",
        Mode::Command => "COMMAND",
    };

    let status_text = format!(
        " {} | {} | Messages: {} | Ctrl+C or :q to quit | ? for help",
        app.status,
        mode_str,
        app.messages.len()
    );

    let status = Paragraph::new(Text::from(status_text))
        .style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::DarkGray),
        );

    frame.render_widget(status, area);
}

fn render_help(frame: &mut Frame, area: Rect) {
    let help_text = vec![
        Line::from(Span::styled(" Help ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from("  ?           Toggle this help screen"),
        Line::from("  :           Enter command mode"),
        Line::from("  Esc         Clear input / exit command mode"),
        Line::from(""),
        Line::from("  Commands:"),
        Line::from("    :q, :quit  Quit whycode"),
        Line::from("    :h, :help  Show this help"),
        Line::from(""),
        Line::from("  Ctrl+C      Quit"),
        Line::from("  ↑↓          Scroll message history"),
        Line::from(""),
        Line::from(Span::styled(" Press Esc, q, or ? to close help ", Style::default().fg(Color::Yellow))),
    ];

    let help_block = Paragraph::new(Text::from(help_text))
        .block(
            Block::default()
                .title(" Help ")
                .borders(Borders::ALL)
                .style(Style::default().bg(Color::Rgb(30, 30, 30))),
        )
        .wrap(Wrap { trim: true });

    // Center the help popup
    let help_area = centered_rect(60, 70, area);
    frame.render_widget(ratatui::widgets::Clear, help_area);
    frame.render_widget(help_block, help_area);
}

/// Helper function to create a centered rectangle
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
