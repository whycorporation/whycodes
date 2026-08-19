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
                ChatBlock::Subagent {
                    kind,
                    description,
                    status,
                    ..
                } => {
                    lines.push(Line::from(Span::styled(
                        format!("  ◆ Subagent {status} ({kind}): {description}"),
                        Style::default().fg(palette.accent),
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

    let elapsed = t.format_elapsed();
    let mut header_spans: Vec<Span<'static>> = if t.is_running() {
        vec![Span::styled("Thinking...".to_string(), label)]
    } else {
        vec![
            Span::styled("Thought".to_string(), label),
            Span::styled(format!(" for {elapsed}"), detail),
        ]
    };
    if t.is_running() && !elapsed.is_empty() && elapsed != "0.0s" {
        header_spans.push(Span::styled(format!("  {elapsed}"), detail));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{ChatBlock, ChatRole, ThinkingBlock};
    use crate::theme::ThemeName;

    fn palette() -> ThemePalette {
        ThemeName::DefaultDark.palette()
    }

    fn line_text(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn widget(role: ChatRole, content: &str, blocks: Vec<ChatBlock>) -> MessageWidget {
        MessageWidget {
            role,
            content: content.to_string(),
            blocks,
            results_expanded: true,
            error: None,
        }
    }

    #[test]
    fn role_headers_use_prefix_and_role_style() {
        let p = palette();
        for (role, prefix, color) in [
            (ChatRole::User, "▸ ", p.user_msg),
            (ChatRole::Assistant, "○ ", p.assistant_msg),
            (ChatRole::System, "⚙ ", p.system_msg),
            (ChatRole::Tool, "  ↪ ", p.tool_msg),
        ] {
            let w = widget(role.clone(), "", vec![]);
            let lines = w.to_lines(&p);
            assert_eq!(
                line_text(&lines[0]),
                format!("{prefix}{}", role.as_str()),
                "{role:?}"
            );
            // Role header carries the role color + bold.
            let span = &lines[0].spans[0];
            assert_eq!(span.style.fg, Some(color), "{role:?}");
            assert!(span.style.add_modifier.contains(Modifier::BOLD));
            // Trailing blank separator line.
            assert_eq!(line_text(lines.last().unwrap()), "", "{role:?}");
        }
    }

    #[test]
    fn content_lines_are_indented() {
        let w = widget(ChatRole::User, "first\nsecond", vec![]);
        let lines = w.to_lines(&palette());
        assert_eq!(line_text(&lines[1]), "  first");
        assert_eq!(line_text(&lines[2]), "  second");
        // Regular content uses the neutral foreground.
        assert_eq!(lines[1].spans[0].style.fg, Some(palette().fg));
    }

    #[test]
    fn text_and_tool_use_blocks() {
        let w = widget(
            ChatRole::Assistant,
            "",
            vec![
                ChatBlock::Text("block line".into()),
                ChatBlock::ToolUse {
                    id: "t1".into(),
                    name: "read".into(),
                    input: serde_json::json!({ "path": "a.rs" }),
                },
            ],
        );
        let lines = w.to_lines(&palette());
        let all: Vec<String> = lines.iter().map(line_text).collect();
        assert!(all.iter().any(|l| l == "  block line"), "{all:?}");
        assert!(
            all.iter()
                .any(|l| l.contains("🔧 read") && l.contains("\"path\": \"a.rs\"")),
            "tool use shows name + pretty JSON: {all:?}"
        );
    }

    #[test]
    fn tool_result_truncated_and_error_colored() {
        let p = palette();
        // Long content is capped with a notice.
        let long = "x".repeat(600);
        let w = widget(
            ChatRole::Tool,
            "",
            vec![ChatBlock::ToolResult {
                id: "r1".into(),
                content: long.clone(),
                is_error: false,
            }],
        );
        let lines = w.to_lines(&p);
        let result_line = lines[1].clone();
        let text = line_text(&result_line);
        assert!(text.starts_with("    ↳ "), "{text}");
        assert!(text.ends_with("... (truncated)"), "len={}", text.len());
        assert!(text.len() < 600, "capped, not full content");
        // Error results use the error color.
        let w = widget(
            ChatRole::Tool,
            "",
            vec![ChatBlock::ToolResult {
                id: "r2".into(),
                content: "boom".into(),
                is_error: true,
            }],
        );
        let lines = w.to_lines(&p);
        assert_eq!(
            lines[1].spans[0].style.fg,
            Some(p.error),
            "error result colored"
        );
        assert!(lines[1].spans[0].style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn error_line_appended_with_error_color() {
        let mut w = widget(ChatRole::Assistant, "hi", vec![]);
        w.error = Some("call failed".into());
        let lines = w.to_lines(&palette());
        let err_line = &lines[lines.len() - 2];
        assert_eq!(line_text(err_line), "  ✗ call failed");
        assert_eq!(
            err_line.spans[0].style.fg,
            Some(palette().error),
            "error line colored"
        );
    }

    #[test]
    fn thinking_running_shows_live_tail() {
        let p = palette();
        let t = ThinkingBlock::new("one\ntwo\nthree\nfour\nfive\nsix");
        // Running + collapsed → header + last 3 lines as the live tail.
        let lines = thinking_widget_lines(&t, &p);
        let all: Vec<String> = lines.iter().map(line_text).collect();
        assert!(all[0].contains("Thinking"), "{all:?}");
        assert!(all.iter().any(|l| l.contains("four")), "{all:?}");
        assert!(all.iter().any(|l| l.contains("six")), "{all:?}");
        assert!(!all.iter().any(|l| l.contains("one")), "tail only: {all:?}");
        // Rail glyph on every painted line.
        assert!(all.iter().all(|l| l.contains('┃')), "{all:?}");
    }

    #[test]
    fn thinking_finished_collapsed_has_no_trailing_chevron() {
        let p = palette();
        let mut t = ThinkingBlock::new("body line");
        t.finish();
        let lines = thinking_widget_lines(&t, &p);
        let all: Vec<String> = lines.iter().map(line_text).collect();
        assert!(all[0].contains("Thought for"), "{all:?}");
        assert!(
            !all[0].contains('›') && !all[0].contains('>'),
            "no trailing chevron: {all:?}"
        );
        assert_eq!(all.len(), 1, "no body when collapsed: {all:?}");
    }

    #[test]
    fn thinking_finished_expanded_shows_full_body() {
        let p = palette();
        let mut t = ThinkingBlock::new("line a\nline b");
        t.collapsed = false;
        t.finish();
        let lines = thinking_widget_lines(&t, &p);
        let all: Vec<String> = lines.iter().map(line_text).collect();
        assert!(all[0].contains("Thought for"), "{all:?}");
        assert!(!all[0].contains('›'), "expanded: {all:?}");
        assert!(all.iter().any(|l| l.contains("line a")), "{all:?}");
        assert!(all.iter().any(|l| l.contains("line b")), "{all:?}");
    }

    #[test]
    fn thinking_header_label_matches_lifecycle() {
        let mut t = ThinkingBlock::new("x");
        // Freshly started → "Thinking…" (0.0s not worth showing).
        assert!(
            t.header_label().starts_with("Thinking"),
            "{}",
            t.header_label()
        );
        t.finish();
        assert!(
            t.header_label().starts_with("Thought for"),
            "{}",
            t.header_label()
        );
    }
}
