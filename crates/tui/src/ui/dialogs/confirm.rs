// ── ui/dialogs/confirm.rs: Confirmation + permission dialogs ───────────
// Grok-style ModalWindow chrome + y/n footer shortcuts.
// Permission prompts get a structured layout (tool · body · risk).

use crate::theme::ThemePalette;
use crate::widgets::wrap::wrap_text;
use ratatui::{
    Frame,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
};

use super::base::DialogChrome;

pub fn render_confirm_dialog(
    frame: &mut Frame,
    title: &str,
    message: &str,
    palette: &ThemePalette,
    mouse_pos: Option<(u16, u16)>,
) -> DialogChrome {
    let chrome = super::base::dialog_frame_sized(
        frame,
        title,
        &["y yes", "n no", "Enter confirm", "Esc / [✗]"],
        palette,
        super::base::DialogSizing::compact(),
        mouse_pos,
        super::base::DialogPlacement::Center,
    );
    let area = chrome.content;
    if area.width == 0 || area.height == 0 {
        return chrome;
    }

    let mut lines = vec![Line::from("")];
    for para in message.split('\n') {
        lines.push(Line::from(Span::styled(
            para.to_string(),
            Style::default().fg(palette.fg),
        )));
    }
    let body = Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: false })
        .style(Style::default().bg(palette.bg));
    frame.render_widget(body, area);
    chrome
}

/// Tool permission prompt: clear hierarchy instead of a raw JSON blob.
///
/// Layout:
/// ```text
///   Tool   bash
///
///   $ ls -la /tmp
///     …
///
///   Risk   may delete files
///
///   Allow this tool to run?
/// ```
pub fn render_permission_dialog(
    frame: &mut Frame,
    tool_name: &str,
    detail: &str,
    palette: &ThemePalette,
    mouse_pos: Option<(u16, u16)>,
) -> DialogChrome {
    let chrome = super::base::dialog_frame_sized(
        frame,
        "Permission required",
        &["y/a allow", "n/d deny", "Esc / [✗]"],
        palette,
        super::base::DialogSizing::popup(),
        mouse_pos,
        super::base::DialogPlacement::Center,
    );
    let area = chrome.content;
    if area.width == 0 || area.height == 0 {
        return chrome;
    }

    let parsed = parse_permission_detail(detail);
    let mut lines: Vec<Line> = Vec::new();

    // Tool row
    lines.push(Line::from(vec![
        Span::styled(
            "Tool  ",
            Style::default()
                .fg(palette.dim)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            tool_name.to_string(),
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(""));

    // Body — command / arguments
    if !parsed.body.is_empty() {
        let body_label = if parsed.is_command {
            "Command"
        } else {
            "Details"
        };
        lines.push(Line::from(Span::styled(
            body_label.to_string(),
            Style::default()
                .fg(palette.dim)
                .add_modifier(Modifier::BOLD),
        )));

        let mut command_prefix_used = false;
        for line in parsed.body.lines() {
            if parsed.is_command {
                if line.is_empty() {
                    lines.push(Line::from(""));
                    continue;
                }
                let (prefix, style_prefix) = if !command_prefix_used {
                    command_prefix_used = true;
                    ("  $ ", Style::default().fg(palette.dim))
                } else {
                    ("    ", Style::default().fg(palette.dim))
                };
                lines.push(Line::from(vec![
                    Span::styled(prefix.to_string(), style_prefix),
                    Span::styled(
                        line.to_string(),
                        Style::default().fg(palette.fg).add_modifier(Modifier::BOLD),
                    ),
                ]));
            } else {
                lines.push(Line::from(Span::styled(
                    format!("  {line}"),
                    Style::default().fg(palette.fg),
                )));
            }
        }
        lines.push(Line::from(""));
    }

    // Risk callout
    if let Some(risk) = &parsed.risk {
        lines.push(Line::from(vec![
            Span::styled(
                "Risk  ",
                Style::default()
                    .fg(palette.error)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(risk.clone(), Style::default().fg(palette.error)),
        ]));
        lines.push(Line::from(""));
    }

    lines.push(Line::from(Span::styled(
        "Allow this tool to run?",
        Style::default().fg(palette.dim),
    )));

    // Wrap first, then drop overflow so a single long command (or many
    // wrapped rows) still leaves the footer readable.
    let wrap_w = area.width.max(1);
    let mut wrapped: Vec<Line> = Vec::new();
    for line in lines {
        let spans = line.spans;
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        if text.is_empty() {
            wrapped.push(Line::from(""));
            continue;
        }
        let style = spans
            .first()
            .map(|s| s.style)
            .unwrap_or_else(Style::default);
        for row in wrap_text(&text, wrap_w) {
            let slice = &text[row.byte_range.0..row.byte_range.1];
            wrapped.push(Line::from(Span::styled(slice.to_string(), style)));
        }
    }

    let max_rows = area.height as usize;
    if wrapped.len() > max_rows {
        let keep = max_rows.saturating_sub(1);
        wrapped.truncate(keep);
        wrapped.push(Line::from(Span::styled(
            "  … (truncated)",
            Style::default().fg(palette.dim),
        )));
    }

    let body = Paragraph::new(Text::from(wrapped)).style(Style::default().bg(palette.bg));
    frame.render_widget(body, area);
    chrome
}

struct ParsedPermission {
    body: String,
    risk: Option<String>,
    is_command: bool,
}

/// Split agent-formatted detail into body + optional risk line.
///
/// Recognizes:
/// - `Command:\n…\n\nRisk: …` (shell risk confirm)
/// - plain multi-line key:value detail
/// - bare command string (single line, no key prefix)
fn parse_permission_detail(detail: &str) -> ParsedPermission {
    let detail = detail.trim();
    if detail.is_empty() {
        return ParsedPermission {
            body: String::new(),
            risk: None,
            is_command: false,
        };
    }

    // Split off trailing `Risk: …` (last section after blank line).
    let (main, risk) = split_risk_section(detail);

    let main = main.trim();
    // `Command:\n…`
    if let Some(rest) = main.strip_prefix("Command:") {
        let body = rest.trim_start_matches(['\n', '\r', ' ']).to_string();
        return ParsedPermission {
            body,
            risk,
            is_command: true,
        };
    }

    // Bare shell-ish single line without labeled keys → treat as command.
    let is_command =
        !main.contains('\n') && !main.contains(": ") && !main.starts_with('{') && !main.is_empty();

    ParsedPermission {
        body: main.to_string(),
        risk,
        is_command,
    }
}

fn split_risk_section(detail: &str) -> (String, Option<String>) {
    // Prefer blank-line-separated trailing "Risk: …"
    if let Some(idx) = detail.rfind("\n\nRisk:") {
        let body = detail[..idx].to_string();
        let risk = detail[idx + "\n\nRisk:".len()..].trim().to_string();
        return (body, if risk.is_empty() { None } else { Some(risk) });
    }
    if let Some(rest) = detail.strip_prefix("Risk:") {
        return (String::new(), Some(rest.trim().to_string()));
    }
    // Inline last line "Risk: …"
    if let Some((head, tail)) = detail.rsplit_once('\n')
        && let Some(r) = tail.strip_prefix("Risk:")
    {
        return (head.to_string(), Some(r.trim().to_string()));
    }
    (detail.to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_shell_risk_detail() {
        let p = parse_permission_detail("Command:\nrm -rf /tmp/x\n\nRisk: destructive delete");
        assert_eq!(p.body, "rm -rf /tmp/x");
        assert_eq!(p.risk.as_deref(), Some("destructive delete"));
        assert!(p.is_command);
    }

    #[test]
    fn parse_bare_command() {
        let p = parse_permission_detail("ls -la");
        assert_eq!(p.body, "ls -la");
        assert!(p.is_command);
        assert!(p.risk.is_none());
    }

    #[test]
    fn parse_key_value_detail() {
        let p = parse_permission_detail("path: src/main.rs\noffset: 10");
        assert!(!p.is_command);
        assert!(p.body.contains("path: src/main.rs"));
        assert!(p.risk.is_none());
    }
}
