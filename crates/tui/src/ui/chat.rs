// ── ui/chat.rs: OpenCode session message list ──────────────────────────
// UserMessage: left ┃ border + panel bg (session/index.tsx UserMessage)
// Assistant: free parts + "▣ agent · model" epilogue
// Home: centered dual-block logo (home.tsx + logo.tsx)

use crate::app::{ChatBlock, ChatRole, TuiApp};
use crate::opencode_tokens::{LOGO_WHY, LOGO_WHY_CODE, layout};
use crate::theme::ThemePalette;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Padding, Paragraph, Wrap},
};

pub fn render(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    if app.messages.is_empty() {
        render_home(frame, area, app, palette);
        return;
    }
    render_session(frame, area, app, palette);
}

fn render_home(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    let mut lines: Vec<Line> = Vec::new();
    // Vertical centering like home.tsx flexGrow spacers
    let content_h = 4 + 1 + 2 + 2; // logo + gap + meta + hints
    let top = area.height.saturating_sub(content_h) / 2;
    for _ in 0..top {
        lines.push(Line::from(""));
    }

    // Center logo horizontally
    let logo_w = LOGO_WHY[1].chars().count() + 1 + LOGO_WHY_CODE[1].chars().count();
    let left_pad = area
        .width
        .saturating_sub(logo_w as u16 + 2)
        .saturating_div(2) as usize;
    let pad = " ".repeat(left_pad);

    for i in 0..4 {
        lines.push(Line::from(vec![
            Span::raw(pad.clone()),
            Span::styled(LOGO_WHY[i].to_string(), Style::default().fg(palette.dim)),
            Span::raw(" "),
            Span::styled(
                LOGO_WHY_CODE[i].to_string(),
                Style::default().fg(palette.fg).add_modifier(Modifier::BOLD),
            ),
        ]));
    }

    lines.push(Line::from(""));
    let agent_color = palette.agent_color_by_index(app.agent_cycle_idx);
    let meta_part = format!(
        "  ·  {}/{}",
        empty_dash(&app.provider_name),
        empty_dash(&app.model_name)
    );
    lines.push(center_line_colored(
        &app.agent_name,
        &meta_part,
        area.width,
        agent_color,
        palette.dim,
        false,
    ));
    lines.push(center_line(
        &app.project_label,
        area.width,
        palette.dim,
        false,
    ));
    lines.push(Line::from(""));
    // "Get started /connect" like footer welcome
    let gs = "Get started  /connect".to_string();
    lines.push(center_line(&gs, area.width, palette.fg, false));

    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().bg(palette.bg)),
        area,
    );
}

fn render_session(frame: &mut Frame, area: Rect, app: &TuiApp, palette: &ThemePalette) {
    let mut lines: Vec<Line> = Vec::new();
    let msg_count = app.messages.len();
    let scroll_offset = app.scroll_offset.min(msg_count.saturating_sub(1));
    let visible_end = msg_count.saturating_sub(scroll_offset);

    // OpenCode scrollbox starts with height={1} spacer
    lines.push(Line::from(""));

    for (i, msg) in app.messages.iter().enumerate() {
        if i >= visible_end {
            break;
        }
        lines.extend(render_message(msg, app, palette, i));
    }

    if scroll_offset > 0 {
        lines.push(Line::from(Span::styled(
            format!("  ↑ {scroll_offset} more · End"),
            Style::default().fg(palette.dim),
        )));
    }

    // Pad left/right like session paddingLeft=2 paddingRight=2
    let block = Block::default()
        .style(Style::default().bg(palette.bg))
        .padding(Padding::horizontal(layout::SIDE_PAD));

    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_message(
    msg: &crate::app::ChatMessage,
    app: &TuiApp,
    palette: &ThemePalette,
    index: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    // marginTop=1 between messages (OpenCode)
    if index > 0 {
        lines.push(Line::from(""));
    }

    match msg.role {
        ChatRole::User => {
            // UserMessage: left border ┃ + panel background — uses agent color
            let border_c = palette.agent_color_by_index(app.agent_cycle_idx);
            let body_lines: Vec<&str> = msg.content.lines().collect();
            let empty = body_lines.is_empty();

            // top padding line inside panel
            lines.push(panel_line(" ", border_c, palette, true));
            if empty {
                lines.push(panel_line("", border_c, palette, false));
            } else {
                for line in body_lines {
                    lines.push(panel_line(line, border_c, palette, false));
                }
            }
            // bottom padding
            lines.push(panel_line(" ", border_c, palette, true));
        }
        ChatRole::Assistant => {
            // Assistant content — no outer box. Rendered as markdown, which
            // also handles partially streamed text (an unterminated fence stays
            // an open block rather than leaking backticks).
            if !msg.content.is_empty() {
                lines.extend(super::markdown::render(&msg.content, palette));
            }
            for block in &msg.blocks {
                match block {
                    ChatBlock::Text(t) if msg.content.is_empty() => {
                        lines.extend(super::markdown::render(t, palette));
                    }
                    ChatBlock::Text(_) => {}
                    ChatBlock::Thinking(t) => {
                        if msg.thinking_collapsed {
                            lines.push(Line::from(vec![
                                Span::raw(" "),
                                Span::styled(
                                    String::from("thinking…"),
                                    Style::default()
                                        .fg(palette.thinking)
                                        .add_modifier(Modifier::DIM | Modifier::ITALIC),
                                ),
                            ]));
                        } else {
                            for line in t.lines().take(12) {
                                lines.push(Line::from(vec![
                                    Span::raw("   "),
                                    Span::styled(
                                        line.to_string(),
                                        Style::default()
                                            .fg(palette.thinking)
                                            .add_modifier(Modifier::DIM | Modifier::ITALIC),
                                    ),
                                ]));
                            }
                        }
                    }
                    ChatBlock::ToolUse { name, input, .. } => {
                        lines.extend(tool_block(name, input, None, false, palette));
                    }
                    ChatBlock::ToolResult {
                        content, is_error, ..
                    } => {
                        lines.extend(tool_result(content, *is_error, palette));
                    }
                }
            }
            for tc in &msg.tool_calls {
                let dup = msg
                    .blocks
                    .iter()
                    .any(|b| matches!(b, ChatBlock::ToolUse { id, .. } if id == &tc.id));
                if dup {
                    if let Some(ref r) = tc.result {
                        lines.extend(tool_result(r, tc.is_error, palette));
                    }
                } else {
                    lines.extend(tool_block(
                        &tc.name,
                        &tc.arguments,
                        tc.result.as_deref(),
                        tc.is_error,
                        palette,
                    ));
                }
            }
            // Epilogue: ▣ agent · model  (AssistantMessage final line)
            let agent_color = palette.agent_color_by_index(app.agent_cycle_idx);
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::raw("   "),
                Span::styled("▣ ".to_string(), Style::default().fg(agent_color)),
                Span::styled(
                    format!("{} ", app.agent_name),
                    Style::default()
                        .fg(agent_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(
                        "· {}/{}",
                        empty_dash(&app.provider_name),
                        empty_dash(&app.model_name)
                    ),
                    Style::default().fg(palette.dim),
                ),
            ]));
        }
        ChatRole::System => {
            for line in msg.content.lines() {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(line.to_string(), Style::default().fg(palette.dim)),
                ]));
            }
        }
        ChatRole::Tool => {
            lines.extend(tool_result(&msg.content, false, palette));
        }
    }

    if let Some(ref err) = msg.error {
        // error box with left border in error color
        lines.push(Line::from(""));
        lines.push(panel_line(err, palette.error, palette, false));
    }

    lines
}

/// OpenCode user panel line: `┃` + pad + text on panel bg
fn panel_line(
    text: &str,
    border: ratatui::style::Color,
    palette: &ThemePalette,
    blank: bool,
) -> Line<'static> {
    Line::from(vec![
        Span::styled("┃".to_string(), Style::default().fg(border)),
        Span::styled(
            if blank {
                "  ".to_string()
            } else {
                format!("  {text}")
            },
            Style::default().fg(palette.fg).bg(palette.status_bar_bg), // STEP2 panel
        ),
    ])
}

fn tool_block(
    name: &str,
    input: &serde_json::Value,
    result: Option<&str>,
    is_error: bool,
    palette: &ThemePalette,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let color = if is_error {
        palette.error
    } else {
        palette.tool_msg
    };
    let summary = tool_summary(input);
    // paddingLeft=3 style
    lines.push(Line::from(vec![
        Span::raw("   "),
        Span::styled("⚙ ".to_string(), Style::default().fg(color)),
        Span::styled(
            name.to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(summary, Style::default().fg(palette.dim)),
    ]));
    if let Some(r) = result {
        lines.extend(tool_result(r, is_error, palette));
    }
    lines
}

fn tool_result(content: &str, is_error: bool, palette: &ThemePalette) -> Vec<Line<'static>> {
    let color = if is_error { palette.error } else { palette.dim };
    let mut lines = Vec::new();
    let total = content.lines().count();
    for line in content.lines().take(8) {
        lines.push(Line::from(vec![
            Span::raw("   "),
            Span::styled("┃ ".to_string(), Style::default().fg(palette.border)),
            Span::styled(line.to_string(), Style::default().fg(color)),
        ]));
    }
    if total > 8 {
        lines.push(Line::from(vec![
            Span::raw("   "),
            Span::styled("┃ ".to_string(), Style::default().fg(palette.border)),
            Span::styled(
                format!("… {} more lines", total - 8),
                Style::default().fg(palette.dim).add_modifier(Modifier::DIM),
            ),
        ]));
    }
    lines
}

fn tool_summary(input: &serde_json::Value) -> String {
    let s = input
        .get("command")
        .or_else(|| input.get("path"))
        .or_else(|| input.get("file_path"))
        .or_else(|| input.get("file"))
        .or_else(|| input.get("pattern"))
        .or_else(|| input.get("glob"))
        .or_else(|| input.get("query"))
        .or_else(|| input.get("goal"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let s = if s.is_empty() {
        let raw = input.to_string();
        if raw.len() > 56 {
            format!("{}…", &raw[..56])
        } else {
            raw
        }
    } else {
        s
    };
    if s.chars().count() > 72 {
        format!("{}…", s.chars().take(71).collect::<String>())
    } else {
        s
    }
}

fn center_line(text: &str, width: u16, color: ratatui::style::Color, bold: bool) -> Line<'static> {
    let w = text.chars().count() as u16;
    let pad = width.saturating_sub(w) / 2;
    let mut style = Style::default().fg(color);
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    Line::from(vec![
        Span::raw(" ".repeat(pad as usize)),
        Span::styled(text.to_string(), style),
    ])
}

/// Centered line with two color segments: first part in `color1`, rest in `color2`.
fn center_line_colored(
    text1: &str,
    text2: &str,
    width: u16,
    color1: ratatui::style::Color,
    color2: ratatui::style::Color,
    bold: bool,
) -> Line<'static> {
    let total_w = (text1.chars().count() + text2.chars().count()) as u16;
    let pad = width.saturating_sub(total_w) / 2;
    let mut style1 = Style::default().fg(color1);
    let mut style2 = Style::default().fg(color2);
    if bold {
        style1 = style1.add_modifier(Modifier::BOLD);
        style2 = style2.add_modifier(Modifier::BOLD);
    }
    Line::from(vec![
        Span::raw(" ".repeat(pad as usize)),
        Span::styled(text1.to_string(), style1),
        Span::styled(text2.to_string(), style2),
    ])
}

fn empty_dash(s: &str) -> &str {
    if s.is_empty() { "—" } else { s }
}
