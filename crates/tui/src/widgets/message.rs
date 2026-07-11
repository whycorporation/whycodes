// ── widgets/message.rs: Message renderer ───────────────────────────────

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use crate::app::{ChatBlock, ChatRole};
use crate::theme::ThemePalette;

/// A display-ready message struct for widget rendering.
#[derive(Debug, Clone)]
pub struct MessageWidget {
    pub role: ChatRole,
    pub content: String,
    pub blocks: Vec<ChatBlock>,
    pub thinking_collapsed: bool,
    pub results_expanded: bool,
    pub error: Option<String>,
}

impl MessageWidget {
    /// Render into lines suitable for a Paragraph widget.
    pub fn to_lines(&self, palette: &ThemePalette) -> Vec<Line<'static>> {
        let mut lines: Vec<Line> = Vec::new();

        let role_style = match self.role {
            ChatRole::User => Style::default().fg(palette.user_msg).add_modifier(Modifier::BOLD),
            ChatRole::Assistant => Style::default().fg(palette.assistant_msg).add_modifier(Modifier::BOLD),
            ChatRole::System => Style::default().fg(palette.system_msg).add_modifier(Modifier::BOLD),
            ChatRole::Tool => Style::default().fg(palette.tool_msg).add_modifier(Modifier::BOLD),
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
                ChatBlock::Thinking(text) => {
                    if self.thinking_collapsed {
                        lines.push(Line::from(Span::styled(
                            "  💭 Thinking... (Enter to expand)",
                            Style::default().fg(palette.thinking),
                        )));
                    } else {
                        lines.push(Line::from(Span::styled(
                            "  💭 Thinking:",
                            Style::default().fg(palette.thinking).add_modifier(Modifier::BOLD),
                        )));
                        for line in text.lines() {
                            lines.push(Line::from(Span::styled(
                                format!("    {}", line),
                                Style::default().fg(palette.thinking).add_modifier(Modifier::DIM),
                            )));
                        }
                    }
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
                    let color = if *is_error { palette.error } else { palette.tool_msg };
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
