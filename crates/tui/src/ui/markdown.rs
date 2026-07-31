//! Render parsed markdown into ratatui lines.
//!
//! `whycode_format::render_markdown` emits ANSI escapes, which ratatui does not
//! interpret — they would reach the screen as literal bytes. This module
//! consumes the structured parse instead and produces styled spans, so markdown
//! picks up the active theme rather than syntect's built-in colours.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use whycode_format::markdown::{Block, Inline, highlight_code_spans, parse_markdown};

use crate::theme::ThemePalette;

/// Left margin applied to every rendered line, matching the plain-text path it
/// replaces.
const INDENT: &str = " ";

/// Render `text` as markdown.
pub fn render(text: &str, palette: &ThemePalette) -> Vec<Line<'static>> {
    parse_markdown(text)
        .iter()
        .flat_map(|block| render_block(block, palette))
        .collect()
}

fn render_block(block: &Block, palette: &ThemePalette) -> Vec<Line<'static>> {
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
            let mut out = vec![Span::raw(INDENT)];
            out.extend(spans.iter().map(|s| inline_span(s, palette, Some(style))));
            vec![Line::from(out)]
        }

        Block::Paragraph(spans) => {
            let mut out = vec![Span::raw(INDENT)];
            out.extend(spans.iter().map(|s| inline_span(s, palette, None)));
            vec![Line::from(out)]
        }

        Block::ListItem { indent, spans } => {
            let mut out = vec![
                Span::raw(format!("{INDENT}{}", " ".repeat(*indent))),
                Span::styled("• ".to_string(), Style::default().fg(palette.accent)),
            ];
            out.extend(spans.iter().map(|s| inline_span(s, palette, None)));
            vec![Line::from(out)]
        }

        Block::Code {
            language,
            lines,
            closed,
        } => render_code(language.as_deref(), lines, *closed, palette),
    }
}

fn render_code(
    language: Option<&str>,
    lines: &[String],
    closed: bool,
    palette: &ThemePalette,
) -> Vec<Line<'static>> {
    let gutter = Style::default().fg(palette.dim);
    let mut out = Vec::with_capacity(lines.len() + 2);

    // A header naming the language, so an unhighlighted block is still
    // identifiable as code.
    out.push(Line::from(vec![
        Span::raw(INDENT),
        Span::styled("┌ ".to_string(), gutter),
        Span::styled(language.unwrap_or("code").to_string(), gutter),
    ]));

    let highlighted = highlight_code_spans(&lines.join("\n"), language);
    for spans in &highlighted {
        let mut line = vec![Span::raw(INDENT), Span::styled("│ ".to_string(), gutter)];
        for ((r, g, b), text) in spans {
            line.push(Span::styled(
                text.trim_end_matches('\n').to_string(),
                Style::default().fg(Color::Rgb(*r, *g, *b)),
            ));
        }
        out.push(Line::from(line));
    }

    // While streaming, the closing fence has not arrived. Leave the block open
    // rather than drawing a bottom edge that will move on the next frame.
    if closed {
        out.push(Line::from(vec![
            Span::raw(INDENT),
            Span::styled("└".to_string(), gutter),
        ]));
    }
    out
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
    fn plain_text_passes_through_unchanged() {
        assert_eq!(rendered("just words").len(), 1);
        assert!(rendered("just words")[0].contains("just words"));
    }

    #[test]
    fn empty_input_renders_nothing() {
        assert!(render("", &palette()).is_empty());
    }

    #[test]
    fn every_line_is_indented() {
        for line in render("# T\n\ntext\n\n- item", &palette()) {
            let s = text(&line);
            assert!(s.is_empty() || s.starts_with(' '), "{s:?}");
        }
    }
}
