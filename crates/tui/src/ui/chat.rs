// ── ui/chat.rs: Chat/message view ──────────────────────────────────────

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use crate::app::{ChatBlock, ChatRole, TuiApp};
use crate::theme::ThemePalette;

pub fn render(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    let mut lines: Vec<Line> = Vec::new();
    let msg_count = app.messages.len();
    let scroll_offset = app.scroll_offset;

    // Scroll indicator.
    if scroll_offset > 0 && msg_count > scroll_offset {
        lines.push(Line::from(Span::styled(
            format!("  ↑ {} older messages ↑  ", scroll_offset),
            Style::default().fg(palette.dim),
        )));
        lines.push(Line::from(""));
    }

    let visible_start = if msg_count > scroll_offset {
        msg_count - scroll_offset
    } else {
        0
    };

    for (i, msg) in app.messages.iter().enumerate() {
        if i < visible_start {
            continue;
        }

        let role_style = role_color(msg.role.clone(), palette);

        // Role prefix.
        let prefix = match msg.role {
            ChatRole::User => "▸ ",
            ChatRole::Assistant => "○ ",
            ChatRole::System => "⚙ ",
            ChatRole::Tool => "  ↪ ",
        };
        let role_line = Span::styled(
            format!("{}{}", prefix, msg.role),
            role_style.add_modifier(Modifier::BOLD),
        );

        // Render content blocks.
        let mut block_lines: Vec<Line> = Vec::new();

        // Thinking blocks (collapsible).
        for block in &msg.blocks {
            match block {
                ChatBlock::Text(text) => {
                    for line in text.lines() {
                        block_lines.push(Line::from(Span::styled(
                            format!("  {}", line),
                            Style::default().fg(palette.fg),
                        )));
                    }
                }
                ChatBlock::Thinking(text) => {
                    if msg.thinking_collapsed {
                        block_lines.push(Line::from(Span::styled(
                            "  💭 Thinking... (Enter to expand)",
                            Style::default().fg(palette.thinking),
                        )));
                    } else {
                        block_lines.push(Line::from(Span::styled(
                            "  💭 Thinking:",
                            Style::default().fg(palette.thinking).add_modifier(Modifier::BOLD),
                        )));
                        for line in text.lines() {
                            block_lines.push(Line::from(Span::styled(
                                format!("    {}", line),
                                Style::default().fg(palette.thinking).add_modifier(Modifier::DIM),
                            )));
                        }
                    }
                }
                ChatBlock::ToolUse { id: _, name, input } => {
                    block_lines.push(Line::from(Span::styled(
                        format!("  🔧 {} {}", name, serde_json::to_string_pretty(input).unwrap_or_default()),
                        Style::default().fg(palette.tool_msg),
                    )));
                }
                ChatBlock::ToolResult { id: _, content, is_error } => {
                    let color = if *is_error { palette.error } else { palette.tool_msg };
                    let truncated = if content.len() > 500 {
                        format!("{}... (truncated)", &content[..500])
                    } else {
                        content.clone()
                    };
                    block_lines.push(Line::from(Span::styled(
                        format!("    ↳ {}", truncated),
                        Style::default().fg(color).add_modifier(Modifier::DIM),
                    )));
                }
            }
        }

        // Tool calls.
        for tc in &msg.tool_calls {
            let style = if tc.is_error {
                Style::default().fg(palette.error)
            } else {
                Style::default().fg(palette.tool_msg)
            };

            if tc.collapsed {
                block_lines.push(Line::from(Span::styled(
                    format!("  ▶ {} — {}", tc.name, serde_json::to_string(&tc.arguments).unwrap_or_default()),
                    style,
                )));
            } else {
                block_lines.push(Line::from(Span::styled(
                    format!("  ▼ {}", tc.name),
                    style.add_modifier(Modifier::BOLD),
                )));
                if let Some(ref result) = tc.result {
                    for line in result.lines().take(5) {
                        block_lines.push(Line::from(Span::styled(
                            format!("    │ {}", line),
                            style.add_modifier(Modifier::DIM),
                        )));
                    }
                }
            }
        }

        // Error.
        if let Some(ref err) = msg.error {
            block_lines.push(Line::from(Span::styled(
                format!("  ✗ {}", err),
                Style::default().fg(palette.error),
            )));
        }

        // Assemble.
        lines.push(Line::from(vec![role_line]));
        for bl in block_lines {
            lines.push(Line::from(vec![
                Span::raw("  "),
                bl.into_iter().next().unwrap_or(Span::raw("")),
            ]));
            // If multiline blocks, add continuation.
        }
        lines.push(Line::from(""));
    }

    // Empty state.
    if app.messages.is_empty() {
        let welcome = vec![
            Line::from(Span::styled(" Welcome to whycode ", Style::default().fg(palette.accent).add_modifier(Modifier::BOLD))),
            Line::from(""),
            Line::from(Span::styled(" Type a message and press Enter to chat with your coding agent.", Style::default().fg(palette.dim))),
            Line::from(Span::styled(" Press ? for help, Ctrl+P to select a provider.", Style::default().fg(palette.dim))),
            Line::from(""),
        ];
        lines = welcome;
    }

    // Scroll-to-bottom button.
    if !app.auto_scroll && app.messages.len() > 10 {
        lines.push(Line::from(Span::styled(
            " ▼ Scroll to bottom (End or scroll down) ",
            Style::default().fg(palette.accent).bg(palette.status_bar_bg),
        )));
    }

    let paragraph = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(palette.border))
                .title(" Chat ")
                .style(Style::default().bg(palette.bg)),
        )
        .wrap(Wrap { trim: true });

    frame.render_widget(paragraph, area);
}

fn role_color(role: ChatRole, palette: &ThemePalette) -> Style {
    match role {
        ChatRole::User => Style::default().fg(palette.user_msg),
        ChatRole::Assistant => Style::default().fg(palette.assistant_msg),
        ChatRole::System => Style::default().fg(palette.system_msg),
        ChatRole::Tool => Style::default().fg(palette.tool_msg),
    }
}
