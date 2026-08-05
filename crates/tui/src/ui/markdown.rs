//! Render parsed markdown into ratatui lines.
//!
//! `whycode_format::render_markdown` emits ANSI escapes, which ratatui does not
//! interpret — they would reach the screen as literal bytes. This module
//! consumes the structured parse instead and produces styled spans, so markdown
//! picks up the active theme for prose; fenced code uses Tokyo Night via
//! `whycode_format::highlight`. Fenced `mermaid` / `mmd` blocks render as
//! Unicode box-drawing diagrams via `whycode_format::mermaid`.
//!
//! Prose soft-wraps to `max_width` (Grok-style transcript; no hard overflow).

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use whycode_format::markdown::{Block, Inline, highlight_code_spans, parse_markdown};
use whycode_format::mermaid::{is_mermaid_language, render_mermaid};

use crate::theme::ThemePalette;
use crate::widgets::wrap::wrap_spans;

/// Render `text` as markdown.
///
/// Lines start at content column 0 — the session shell already applies
/// SIDE_PAD. An extra indent here stacked with tools/epilogue and made the
/// transcript look over-nested.
///
/// `max_width` is the content column budget for soft-wrap and Mermaid.
/// Pass `None` when the terminal width is unknown (no wrap).
pub fn render(text: &str, palette: &ThemePalette) -> Vec<Line<'static>> {
    render_with_width(text, palette, None)
}

/// Like [`render`], but soft-wraps prose and passes `max_width` into Mermaid.
pub fn render_with_width(
    text: &str,
    palette: &ThemePalette,
    max_width: Option<usize>,
) -> Vec<Line<'static>> {
    parse_markdown(text)
        .iter()
        .flat_map(|block| render_block(block, palette, max_width))
        .collect()
}

fn render_block(
    block: &Block,
    palette: &ThemePalette,
    max_width: Option<usize>,
) -> Vec<Line<'static>> {
    match block {
        Block::Blank => vec![Line::from("")],

        Block::Heading { level, spans } => {
            // Levels are distinguished by weight rather than by size, which a
            // terminal cannot vary. Deeper headings drop the underline.
            let mut style = Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD);
            if *level <= 2 {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            let out: Vec<Span> = spans
                .iter()
                .map(|s| inline_span(s, palette, Some(style)))
                .collect();
            wrap_prose(out, max_width)
        }

        Block::Paragraph(spans) => {
            let out: Vec<Span> = spans
                .iter()
                .map(|s| inline_span(s, palette, None))
                .collect();
            wrap_prose(out, max_width)
        }

        Block::ListItem {
            indent,
            number,
            spans,
        } => {
            let marker = match number {
                Some(n) => format!("{n}. "),
                None => "• ".to_string(),
            };
            let marker_style = Style::default().fg(palette.accent);
            let prefix = vec![
                Span::raw(" ".repeat(*indent)),
                Span::styled(marker.clone(), marker_style),
            ];
            let body: Vec<Span> = spans
                .iter()
                .map(|s| inline_span(s, palette, None))
                .collect();

            // Soft-wrap body; hang indent under the marker on continuations.
            let marker_cols = indent + marker.chars().count();
            let body_width = max_width.map(|w| w.saturating_sub(marker_cols).max(8));
            let body_lines = wrap_prose(body, body_width);
            let mut out = Vec::new();
            for (i, line) in body_lines.into_iter().enumerate() {
                if i == 0 {
                    let mut spans = prefix.clone();
                    spans.extend(line.spans);
                    out.push(Line::from(spans));
                } else {
                    let mut spans = vec![Span::raw(" ".repeat(marker_cols))];
                    spans.extend(line.spans);
                    out.push(Line::from(spans));
                }
            }
            if out.is_empty() {
                out.push(Line::from(prefix));
            }
            out
        }

        Block::Code {
            language,
            lines,
            closed,
        } => {
            if is_mermaid_language(language.as_deref()) {
                render_mermaid_block(lines, *closed, palette, max_width)
            } else {
                render_code(language.as_deref(), lines, *closed, palette, max_width)
            }
        }
    }
}

fn wrap_prose(spans: Vec<Span<'static>>, max_width: Option<usize>) -> Vec<Line<'static>> {
    match max_width {
        Some(w) if w > 0 => wrap_spans(spans, w as u16),
        _ => vec![Line::from(spans)],
    }
}

fn render_code(
    language: Option<&str>,
    lines: &[String],
    closed: bool,
    palette: &ThemePalette,
    max_width: Option<usize>,
) -> Vec<Line<'static>> {
    let gutter = Style::default().fg(palette.dim);
    let mut out = Vec::with_capacity(lines.len() + 2);

    // Quiet language label (Grok-like; no heavy box chrome noise).
    out.push(Line::from(vec![
        Span::styled("┌ ".to_string(), gutter),
        Span::styled(language.unwrap_or("code").to_string(), gutter),
    ]));

    let body_w = max_width.map(|w| w.saturating_sub(2).max(8));
    let highlighted = highlight_code_spans(&lines.join("\n"), language);
    for spans in highlighted.iter() {
        let mut code_spans: Vec<Span<'static>> = spans
            .iter()
            .map(|((r, g, b), text)| {
                Span::styled(
                    text.trim_end_matches('\n').to_string(),
                    Style::default().fg(Color::Rgb(*r, *g, *b)),
                )
            })
            .collect();
        if code_spans.is_empty() {
            code_spans.push(Span::raw(""));
        }
        // Hard-split only if a single code line still exceeds the pane.
        let rows = match body_w {
            Some(w) => wrap_spans(code_spans, w as u16),
            None => vec![Line::from(code_spans)],
        };
        for row in rows {
            let mut line = vec![Span::styled("│ ".to_string(), gutter)];
            line.extend(row.spans);
            out.push(Line::from(line));
        }
    }

    // While streaming, the closing fence has not arrived. Leave the block open
    // rather than drawing a bottom edge that will move on the next frame.
    if closed {
        out.push(Line::from(vec![Span::styled("└".to_string(), gutter)]));
    }
    out
}

/// Render a ` ```mermaid ` fence as a Unicode diagram when the fence is closed.
///
/// While streaming (`closed == false`) the source is shown as a labelled code
/// block so partial graphs do not thrash the layout on every token. On render
/// failure the source is kept with a dim error note in the header.
fn render_mermaid_block(
    lines: &[String],
    closed: bool,
    palette: &ThemePalette,
    max_width: Option<usize>,
) -> Vec<Line<'static>> {
    let gutter = Style::default().fg(palette.dim);
    let source = lines.join("\n");

    if !closed {
        // Streaming: show source so the user sees progress without paying for
        // a full layout on every partial parse.
        return render_code(Some("mermaid"), lines, false, palette, max_width);
    }

    match render_mermaid(&source, max_width) {
        Ok(diagram) => {
            let mut out = Vec::with_capacity(diagram.len() + 2);
            out.push(Line::from(vec![
                Span::styled("┌ ".to_string(), gutter),
                Span::styled("mermaid".to_string(), gutter),
            ]));
            let body = Style::default().fg(palette.fg);
            for line in diagram.iter() {
                out.push(Line::from(vec![
                    Span::styled("│ ".to_string(), gutter),
                    Span::styled(line.clone(), body),
                ]));
            }
            out.push(Line::from(vec![Span::styled("└".to_string(), gutter)]));
            out
        }
        Err(err) => {
            let mut out = Vec::with_capacity(lines.len() + 2);
            out.push(Line::from(vec![
                Span::styled("┌ ".to_string(), gutter),
                Span::styled(
                    format!("mermaid (render failed: {err})"),
                    Style::default().fg(palette.warning),
                ),
            ]));
            let body = Style::default().fg(palette.fg);
            for line in lines {
                out.push(Line::from(vec![
                    Span::styled("│ ".to_string(), gutter),
                    Span::styled(line.clone(), body),
                ]));
            }
            out.push(Line::from(vec![Span::styled("└".to_string(), gutter)]));
            out
        }
    }
}

/// Style one inline run. `base` lets a heading impose bold on everything inside
/// it while each run keeps its own colour.
fn inline_span(inline: &Inline, palette: &ThemePalette, base: Option<Style>) -> Span<'static> {
    let base = base.unwrap_or_else(|| Style::default().fg(palette.fg));
    match inline {
        Inline::Text(s) => Span::styled(s.clone(), base),
        Inline::Bold(s) => Span::styled(s.clone(), base.add_modifier(Modifier::BOLD)),
        Inline::Italic(s) => Span::styled(s.clone(), base.add_modifier(Modifier::ITALIC)),
        Inline::Code(s) => Span::styled(
            s.clone(),
            Style::default().fg(palette.accent).bg(palette.input_bg),
        ),
        Inline::Link { text, .. } => Span::styled(
            text.clone(),
            Style::default()
                .fg(palette.info)
                .add_modifier(Modifier::UNDERLINED),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeName;

    fn palette() -> ThemePalette {
        ThemeName::DefaultDark.palette()
    }

    /// The visible characters of a line, ignoring style.
    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn rendered(input: &str) -> Vec<String> {
        render(input, &palette()).iter().map(text).collect()
    }

    #[test]
    fn markup_characters_do_not_reach_the_screen() {
        let out = rendered("# Title\n\nsome **bold** and *italic* and `code`\n\n- item");
        let joined = out.join("\n");
        assert!(!joined.contains("**"), "{joined}");
        assert!(!joined.contains('`'), "{joined}");
        assert!(!joined.contains("# "), "{joined}");
        // The content survives.
        assert!(joined.contains("Title"));
        assert!(joined.contains("bold"));
        assert!(joined.contains("italic"));
        assert!(joined.contains("code"));
        assert!(joined.contains("item"));
    }

    #[test]
    fn headings_are_bold() {
        let lines = render("# Title", &palette());
        let styled = lines[0]
            .spans
            .iter()
            .find(|s| s.content == "Title")
            .unwrap();
        assert!(styled.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn bold_and_italic_carry_their_modifiers() {
        let lines = render("a **b** *i*", &palette());
        let spans = &lines[0].spans;
        let bold = spans.iter().find(|s| s.content == "b").unwrap();
        let italic = spans.iter().find(|s| s.content == "i").unwrap();
        assert!(bold.style.add_modifier.contains(Modifier::BOLD));
        assert!(italic.style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn list_items_get_a_bullet() {
        let out = rendered("- first");
        assert!(out[0].contains('•'), "{:?}", out);
        assert!(out[0].contains("first"));
    }

    #[test]
    fn ordered_list_items_get_numbers() {
        let out = rendered("1. alpha\n2. beta");
        assert!(out[0].contains("1. "), "{:?}", out);
        assert!(out[0].contains("alpha"));
        assert!(out[1].contains("2. "), "{:?}", out);
    }

    #[test]
    fn long_paragraph_soft_wraps() {
        let words = (0..20).map(|i| format!("word{i}")).collect::<Vec<_>>().join(" ");
        let lines = render_with_width(&words, &palette(), Some(24));
        assert!(
            lines.len() >= 2,
            "expected wrap into multiple rows, got {}: {:?}",
            lines.len(),
            lines.iter().map(text).collect::<Vec<_>>()
        );
        let joined: String = lines.iter().map(text).collect();
        assert!(joined.contains("word0"));
        assert!(joined.contains("word19"));
    }

    #[test]
    fn fenced_code_is_framed_and_labelled() {
        let out = rendered("```rust\nlet x = 1;\n```");
        assert!(out[0].contains("rust"), "{:?}", out);
        assert!(out.iter().any(|l| l.contains("let x = 1;")), "{:?}", out);
        assert!(out.last().unwrap().contains('└'), "{:?}", out);
    }

    #[test]
    fn an_untagged_fence_still_renders_as_a_block() {
        let out = rendered("```\nplain\n```");
        assert!(out[0].contains("code"), "{:?}", out);
        assert!(out.iter().any(|l| l.contains("plain")));
    }

    #[test]
    fn a_streaming_fence_renders_without_a_bottom_edge() {
        // Partial output during a turn: the closing fence has not arrived.
        let out = rendered("```rust\nlet x = 1;");
        assert!(out.iter().any(|l| l.contains("let x = 1;")), "{:?}", out);
        assert!(
            !out.last().unwrap().contains('└'),
            "an open block should not be closed: {:?}",
            out
        );
    }

    #[test]
    fn mermaid_fence_renders_as_diagram() {
        let out = rendered("```mermaid\ngraph LR; A[Build] --> B[Deploy]\n```");
        let joined = out.join("\n");
        assert!(joined.contains("mermaid"), "{joined}");
        assert!(joined.contains("Build"), "{joined}");
        assert!(joined.contains("Deploy"), "{joined}");
        assert!(out.last().unwrap().contains('└'), "{joined}");
        // With the `mermaid` feature, source keywords become a diagram.
        // Without it, the ship binary keeps source lines readable.
        #[cfg(feature = "mermaid")]
        assert!(!joined.contains("graph LR"), "{joined}");
    }

    #[test]
    fn streaming_mermaid_shows_source_without_closing() {
        let out = rendered("```mermaid\ngraph LR; A --> B");
        let joined = out.join("\n");
        assert!(joined.contains("mermaid"), "{joined}");
        // Still open: no bottom edge yet.
        assert!(
            !out.last().unwrap().contains('└'),
            "open mermaid fence should stay open: {joined}"
        );
    }

    #[test]
    fn plain_text_passes_through_unchanged() {
        assert_eq!(rendered("just words").len(), 1);
        assert!(rendered("just words")[0].contains("just words"));
    }

    #[test]
    fn empty_input_renders_nothing() {
        assert!(render("", &palette()).is_empty());
    }

    #[test]
    fn body_starts_at_content_column() {
        // No synthetic left pad — SIDE_PAD lives in the shell.
        for line in render("# T\n\ntext\n\n- item", &palette()) {
            let s = text(&line);
            if s.is_empty() {
                continue;
            }
            assert!(
                !s.starts_with("  "),
                "body/list should not start with a double space pad: {s:?}"
            );
        }
        assert!(rendered("just words")[0].starts_with('j'));
        assert!(rendered("# Title")[0].starts_with('T'));
    }
}
