// ── widgets/message.rs: Message renderer ───────────────────────────────

use crate::app::{ChatBlock, ChatRole, ThinkingBlock};
use crate::theme::ThemePalette;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

/// A display-ready message struct for widget rendering.
#[derive(Debug, Clone)]
pub struct MessageWidget {
    pub role: ChatRole,
    pub content: String,
    pub blocks: Vec<ChatBlock>,
    pub results_expanded: bool,
    pub error: Option<String>,
}

impl MessageWidget {
    /// Render into lines suitable for a Paragraph widget.
    pub fn to_lines(&self, palette: &ThemePalette) -> Vec<Line<'static>> {
        let mut lines: Vec<Line> = Vec::new();

        let role_style = match self.role {
            ChatRole::User => Style::default()
                .fg(palette.user_msg)
                .add_modifier(Modifier::BOLD),
            ChatRole::Assistant => Style::default()
                .fg(palette.assistant_msg)
                .add_modifier(Modifier::BOLD),
            ChatRole::System => Style::default()
                .fg(palette.system_msg)
                .add_modifier(Modifier::BOLD),
            ChatRole::Tool => Style::default()
                .fg(palette.tool_msg)
                .add_modifier(Modifier::BOLD),
        };

        // Role header.
        let prefix = match self.role {
            ChatRole::User => "▸ ",
            ChatRole::Assistant => "○ ",
            ChatRole::System => "⚙ ",
            ChatRole::Tool => "  ↪ ",
        };
        lines.push(Line::from(Span::styled(
            format!("{}{}", prefix, self.role),
            role_style,
        )));

        // Main content (wrapped).
        if !self.content.is_empty() {
            for line in self.content.lines() {
                lines.push(Line::from(Span::styled(
                    format!("  {}", line),
                    Style::default().fg(palette.fg),
                )));
            }
        }

        // Blocks.
        for block in &self.blocks {
            match block {
                ChatBlock::Text(text) => {
                    for line in text.lines() {
                        lines.push(Line::from(Span::styled(
                            format!("  {}", line),
                            Style::default().fg(palette.fg),
                        )));
                    }
                }
                ChatBlock::Thinking(t) => {
                    lines.extend(thinking_widget_lines(t, palette));
                }
                ChatBlock::ToolUse { id: _, name, input } => {
                    lines.push(Line::from(Span::styled(
                        format!(
                            "  🔧 {} {}",
                            name,
                            serde_json::to_string_pretty(input).unwrap_or_default()
                        ),
                        Style::default().fg(palette.tool_msg),
                    )));
                }
                ChatBlock::ToolResult {
                    id: _,
                    content,
                    is_error,
                } => {
                    let color = if *is_error {
                        palette.error
                    } else {
                        palette.tool_msg
                    };
                    let truncated = if content.len() > 500 {
                        format!("{}... (truncated)", &content[..500])
                    } else {
                        content.clone()
                    };
                    lines.push(Line::from(Span::styled(
                        format!("    ↳ {}", truncated),
                        Style::default().fg(color).add_modifier(Modifier::DIM),
                    )));
                }
            }
        }

        // Error.
        if let Some(ref err) = self.error {
            lines.push(Line::from(Span::styled(
                format!("  ✗ {}", err),
                Style::default().fg(palette.error),
            )));
        }

        lines.push(Line::from(""));
        lines
    }
}

fn thinking_widget_lines(t: &ThinkingBlock, palette: &ThemePalette) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    // Grok-style: muted bold header, dim primary body, full-height ┃ rail.
    let label = Style::default()
        .fg(palette.dim)
        .add_modifier(Modifier::BOLD);
    let detail = Style::default().fg(palette.dim);
    let body = Style::default().fg(palette.fg).add_modifier(Modifier::DIM);
    let show_rail = t.show_body();

    let rail_prefix = |spans: Vec<Span<'static>>| -> Line<'static> {
        let mut out = vec![Span::raw("  ")];
        if show_rail {
            out.push(Span::styled("┃".to_string(), detail));
            out.push(Span::raw(" "));
        }
        out.extend(spans);
        Line::from(out)
    };

    let mut header_spans: Vec<Span<'static>> = if t.is_running() {
        vec![Span::styled("Thinking…".to_string(), label)]
    } else {
        vec![
            Span::styled("Thought".to_string(), label),
            Span::styled(format!(" for {}", t.format_elapsed()), detail),
        ]
    };
    if !t.is_running() && t.collapsed {
        header_spans.push(Span::styled("  (e expand)".to_string(), detail));
    }
    lines.push(rail_prefix(header_spans));
    if show_rail {
        if t.is_truncated_live() {
            lines.push(rail_prefix(vec![Span::styled("…".to_string(), body)]));
        }
        for line in t.body_lines() {
            lines.push(rail_prefix(vec![Span::styled(line.to_string(), body)]));
        }
    }
    lines
}
