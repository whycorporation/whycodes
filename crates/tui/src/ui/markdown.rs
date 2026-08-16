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
use whycode_format::markdown::{Block, Inline, TableAlign, highlight_code_spans, parse_markdown};
use whycode_format::mermaid::{is_mermaid_language, render_mermaid};
use whycode_format::table::{column_widths, pad_cell};

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
            } else if is_diff_language(language.as_deref()) {
                render_diff_code(language.as_deref(), lines, *closed, palette, max_width)
            } else {
                render_code(language.as_deref(), lines, *closed, palette, max_width)
            }
        }

        Block::Table {
            headers,
            aligns,
            rows,
        } => render_table(headers, aligns, rows, palette, max_width),
    }
}

/// GFM pipe table → aligned Unicode box (not soft-wrapped `| raw |` lines).
///
/// ```text
/// ┌────────┬────────┐
/// │ Tag    │ Sürüm  │
/// ├────────┼────────┤
/// │ latest │ 4.5.2  │
/// └────────┴────────┘
/// ```
fn render_table(
    headers: &[String],
    aligns: &[TableAlign],
    rows: &[Vec<String>],
    palette: &ThemePalette,
    max_width: Option<usize>,
) -> Vec<Line<'static>> {
    if headers.is_empty() {
        return Vec::new();
    }
    let col_count = headers.len();
    let widths = column_widths(headers, rows, max_width);
    let border = Style::default().fg(palette.dim);
    let header_style = Style::default()
        .fg(palette.accent)
        .add_modifier(Modifier::BOLD);
    let cell_style = Style::default().fg(palette.fg);

    let mut out = Vec::with_capacity(rows.len() + 4);
    out.push(table_border_line(&widths, BorderKind::Top, border));
    out.push(table_row_line(
        headers,
        &widths,
        aligns,
        col_count,
        header_style,
        border,
    ));
    out.push(table_border_line(&widths, BorderKind::Mid, border));
    for row in rows {
        let cells: Vec<String> = (0..col_count)
            .map(|i| row.get(i).cloned().unwrap_or_default())
            .collect();
        out.push(table_row_line(
            &cells, &widths, aligns, col_count, cell_style, border,
        ));
    }
    out.push(table_border_line(&widths, BorderKind::Bot, border));
    out
}

#[derive(Clone, Copy)]
enum BorderKind {
    Top,
    Mid,
    Bot,
}

fn table_border_line(widths: &[usize], kind: BorderKind, style: Style) -> Line<'static> {
    let (l, m, r, h) = match kind {
        BorderKind::Top => ('┌', '┬', '┐', '─'),
        BorderKind::Mid => ('├', '┼', '┤', '─'),
        BorderKind::Bot => ('└', '┴', '┘', '─'),
    };
    let mut s = String::new();
    s.push(l);
    for (i, w) in widths.iter().enumerate() {
        if i > 0 {
            s.push(m);
        }
        s.extend(std::iter::repeat_n(h, w + 2));
    }
    s.push(r);
    Line::from(Span::styled(s, style))
}

fn table_row_line(
    cells: &[String],
    widths: &[usize],
    aligns: &[TableAlign],
    col_count: usize,
    cell_style: Style,
    border_style: Style,
) -> Line<'static> {
    let mut spans = Vec::with_capacity(col_count * 4 + 1);
    spans.push(Span::styled("│".to_string(), border_style));
    for i in 0..col_count {
        let raw = cells.get(i).map(|s| s.as_str()).unwrap_or("");
        let align = aligns.get(i).copied().unwrap_or(TableAlign::Left);
        let w = widths.get(i).copied().unwrap_or(1);
        let padded = pad_cell(raw, w, align);
        spans.push(Span::styled(" ".to_string(), cell_style));
        spans.push(Span::styled(padded, cell_style));
        spans.push(Span::styled(" ".to_string(), cell_style));
        spans.push(Span::styled("│".to_string(), border_style));
    }
    Line::from(spans)
}

fn is_diff_language(language: Option<&str>) -> bool {
    matches!(
        language.map(|s| s.to_ascii_lowercase()).as_deref(),
        Some("diff" | "patch" | "udiff")
    )
}

fn wrap_prose(spans: Vec<Span<'static>>, max_width: Option<usize>) -> Vec<Line<'static>> {
    match max_width {
        Some(w) if w > 0 => wrap_spans(spans, w as u16),
        _ => vec![Line::from(spans)],
    }
}

/// Grok fenced code: elevated `bg_light` band, dim language chip, numbered
/// gutter. No box-drawing `┌│└` chrome.
fn render_code(
    language: Option<&str>,
    lines: &[String],
    _closed: bool,
    palette: &ThemePalette,
    max_width: Option<usize>,
) -> Vec<Line<'static>> {
    let band = palette.status_bar_bg;
    let gutter = Style::default().fg(palette.dim).bg(band);
    let mut out = Vec::with_capacity(lines.len() + 3);
    out.push(code_band_pad(band));

    if let Some(lang) = language.map(str::trim).filter(|s| !s.is_empty()) {
        out.push(Line::from(vec![
            Span::styled(format!(" {lang} "), gutter),
            Span::styled(" ".to_string(), Style::default().bg(band)),
        ]));
    }

    let n = lines.len().max(1);
    let nw = n.to_string().len().max(2);
    let gutter_w = nw + 2; // " 1 "
    let body_w = max_width.map(|w| w.saturating_sub(gutter_w).max(8));
    let source: Vec<String> = lines.iter().map(|l| l.replace('\t', "    ")).collect();
    let highlighted = highlight_code_spans(&source.join("\n"), language);

    for (i, spans) in highlighted.iter().enumerate() {
        let mut code_spans: Vec<Span<'static>> = spans
            .iter()
            .map(|((r, g, b), text)| {
                Span::styled(
                    text.trim_end_matches('\n').to_string(),
                    Style::default().fg(Color::Rgb(*r, *g, *b)).bg(band),
                )
            })
            .collect();
        if code_spans.is_empty() {
            code_spans.push(Span::styled(" ".to_string(), Style::default().bg(band)));
        }
        let rows = match body_w {
            Some(w) => wrap_spans(code_spans, w as u16),
            None => vec![Line::from(code_spans)],
        };
        let no = format!(" {:>w$} ", i + 1, w = nw);
        let hang = " ".repeat(gutter_w);
        for (j, row) in rows.into_iter().enumerate() {
            let mut line = if j == 0 {
                vec![Span::styled(no.clone(), gutter)]
            } else {
                vec![Span::styled(hang.clone(), Style::default().bg(band))]
            };
            for mut s in row.spans {
                s.style.bg = Some(band);
                line.push(s);
            }
            line.push(Span::styled(" ".to_string(), Style::default().bg(band)));
            out.push(Line::from(line));
        }
    }

    out.push(code_band_pad(band));
    out
}

fn code_band_pad(band: Color) -> Line<'static> {
    Line::from(Span::styled(" ".to_string(), Style::default().bg(band)))
}

/// Fenced `diff` / `patch` blocks: theme add/remove/hunk colours + soft wash
/// (syntect's generic theme misses the semantic red/green of a real diff UI).
fn render_diff_code(
    language: Option<&str>,
    lines: &[String],
    _closed: bool,
    palette: &ThemePalette,
    max_width: Option<usize>,
) -> Vec<Line<'static>> {
    let band = palette.status_bar_bg;
    let gutter = Style::default().fg(palette.dim).bg(band);
    let mut out = Vec::with_capacity(lines.len() + 3);
    out.push(code_band_pad(band));
    let lang = language.unwrap_or("diff");
    out.push(Line::from(vec![
        Span::styled(format!(" {lang} "), gutter),
        Span::styled(" ".to_string(), Style::default().bg(band)),
    ]));

    let add_bg = palette.diff_line_bg(palette.diff_add);
    let rem_bg = palette.diff_line_bg(palette.diff_remove);
    let n = lines.len().max(1);
    let nw = n.to_string().len().max(2);
    let gutter_w = nw + 2;
    let body_w = max_width.map(|w| w.saturating_sub(gutter_w).max(8));

    for (i, raw) in lines.iter().enumerate() {
        let parts = whycode_format::diff::parse_diff_line(raw);
        let (fg, row_bg, bold) = if raw.starts_with("+++") || raw.starts_with("---") {
            (palette.fg, band, false)
        } else if raw.starts_with("@@") || raw.starts_with("diff --git") {
            (palette.diff_hunk, band, false)
        } else if parts.marker == Some('+') {
            (palette.diff_add, add_bg, true)
        } else if parts.marker == Some('-') {
            (palette.diff_remove, rem_bg, true)
        } else {
            (palette.dim, band, false)
        };

        let mut style = Style::default().fg(fg).bg(row_bg);
        if bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        let body = Span::styled(raw.replace('\t', "    "), style);
        let rows = match body_w {
            Some(w) => wrap_spans(vec![body], w as u16),
            None => vec![Line::from(body)],
        };
        let no = format!(" {:>w$} ", i + 1, w = nw);
        let hang = " ".repeat(gutter_w);
        let gstyle = Style::default().fg(palette.dim).bg(row_bg);
        for (j, row) in rows.into_iter().enumerate() {
            let mut line = if j == 0 {
                vec![Span::styled(no.clone(), gstyle)]
            } else {
                vec![Span::styled(hang.clone(), Style::default().bg(row_bg))]
            };
            for mut s in row.spans {
                s.style.bg = Some(row_bg);
                line.push(s);
            }
            line.push(Span::styled(" ".to_string(), Style::default().bg(row_bg)));
            out.push(Line::from(line));
        }
    }

    out.push(code_band_pad(band));
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
    let source = lines.join("\n");

    if !closed {
        // Streaming: show source so the user sees progress without paying for
        // a full layout on every partial parse.
        return render_code(Some("mermaid"), lines, false, palette, max_width);
    }

    let band = palette.status_bar_bg;
    let label = Style::default().fg(palette.dim).bg(band);
    match render_mermaid(&source, max_width) {
        Ok(diagram) => {
            let mut out = Vec::with_capacity(diagram.len() + 3);
            out.push(code_band_pad(band));
            out.push(Line::from(vec![
                Span::styled(" mermaid ".to_string(), label),
                Span::styled(" ".to_string(), Style::default().bg(band)),
            ]));
            let body = Style::default().fg(palette.fg).bg(band);
            for line in diagram.iter() {
                out.push(Line::from(vec![
                    Span::styled(" ".to_string(), body),
                    Span::styled(line.clone(), body),
                    Span::styled(" ".to_string(), Style::default().bg(band)),
                ]));
            }
            out.push(code_band_pad(band));
            out
        }
        Err(err) => {
            let mut out = Vec::with_capacity(lines.len() + 3);
            out.push(code_band_pad(band));
            out.push(Line::from(vec![
                Span::styled(
                    format!(" mermaid (render failed: {err}) "),
                    Style::default().fg(palette.warning).bg(band),
                ),
                Span::styled(" ".to_string(), Style::default().bg(band)),
            ]));
            let body = Style::default().fg(palette.fg).bg(band);
            for line in lines {
                out.push(Line::from(vec![
                    Span::styled(" ".to_string(), body),
                    Span::styled(line.clone(), body),
                    Span::styled(" ".to_string(), Style::default().bg(band)),
                ]));
            }
            out.push(code_band_pad(band));
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
        let words = (0..20)
            .map(|i| format!("word{i}"))
            .collect::<Vec<_>>()
            .join(" ");
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
    fn fenced_rust_uses_more_than_one_token_colour() {
        let lines = render("```rust\nfn main() { let x = \"hi\"; }\n```", &palette());
        let mut fgs = std::collections::BTreeSet::new();
        for line in &lines {
            for s in &line.spans {
                let t = s.content.as_ref();
                if (t.contains("fn") || t.contains("let") || t.contains("hi") || t.contains("main"))
                    && let Some(Color::Rgb(r, g, b)) = s.style.fg
                {
                    fgs.insert((r, g, b));
                }
            }
        }
        assert!(
            fgs.len() >= 2,
            "rust tokens must not share one grey: {fgs:?}"
        );
    }

    #[test]
    fn fenced_code_is_banded_labelled_and_numbered() {
        let out = rendered("```rust\nlet x = 1;\n```");
        let joined = out.join("\n");
        assert!(joined.contains("rust"), "{out:?}");
        assert!(joined.contains("let x = 1;"), "{out:?}");
        assert!(
            joined.contains('1'),
            "Grok code blocks number lines, got {out:?}"
        );
        assert!(
            !joined.contains('┌') && !joined.contains('└'),
            "no box chrome: {out:?}"
        );
        let lines = render("```rust\nlet x = 1;\n```", &palette());
        let banded = lines.iter().any(|l| {
            l.spans
                .iter()
                .any(|s| s.style.bg == Some(palette().status_bar_bg))
        });
        assert!(banded, "code block sits on the elevated band");
    }

    #[test]
    fn gfm_pipe_table_renders_as_aligned_box() {
        let md = "\
| Tag | Sürüm |
|-----|--------|
| latest | 4.5.2 |
| 3x | 3.21.11 |
";
        let out = rendered(md);
        let joined = out.join("\n");
        // Box chrome — not raw pipe-markdown soft-wrap debris.
        assert!(joined.contains('┌'), "{joined}");
        assert!(joined.contains('│'), "{joined}");
        assert!(joined.contains('└'), "{joined}");
        assert!(joined.contains("Tag"), "{joined}");
        assert!(joined.contains("Sürüm"), "{joined}");
        assert!(joined.contains("latest"), "{joined}");
        assert!(joined.contains("4.5.2"), "{joined}");
        // Separator markdown must not leak as a body row.
        assert!(!joined.contains("|-----|"), "{joined}");
        // Header row is accent+bold.
        let lines = render(md, &palette());
        let header_span = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.as_ref().contains("Tag"))
            .expect("header cell");
        assert!(header_span.style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(header_span.style.fg, Some(palette().accent));
    }

    #[test]
    fn fenced_diff_uses_theme_add_remove_colours() {
        let lines = render("```diff\n-old\n+new\n```", &palette());
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("-old"), "{joined}");
        assert!(joined.contains("+new"), "{joined}");

        let add = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains("+new"))
            .expect("add line span");
        let rem = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains("-old"))
            .expect("remove line span");
        assert_eq!(add.style.fg, Some(palette().diff_add));
        assert_eq!(rem.style.fg, Some(palette().diff_remove));
    }

    #[test]
    fn an_untagged_fence_still_renders_as_a_block() {
        let out = rendered("```\nplain\n```");
        assert!(out.iter().any(|l| l.contains("plain")), "{out:?}");
        assert!(
            !out.iter().any(|l| l.trim() == "code"),
            "untagged fences have no fake language chip: {out:?}"
        );
    }

    #[test]
    fn a_streaming_fence_renders_without_a_bottom_edge() {
        // Partial output during a turn: the closing fence has not arrived.
        let out = rendered("```rust\nlet x = 1;");
        assert!(out.iter().any(|l| l.contains("let x = 1;")), "{:?}", out);
        assert!(
            !out.iter().any(|l| l.contains('┌') || l.contains('└')),
            "Grok code blocks have no box edges: {:?}",
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
        assert!(
            !joined.contains('┌') && !joined.contains('└'),
            "mermaid uses the same band, not a box: {joined}"
        );
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
            !joined.contains('┌') && !joined.contains('└'),
            "open mermaid fence should stay a band, not a box: {joined}"
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
