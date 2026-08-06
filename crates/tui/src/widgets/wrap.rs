// ── widgets/wrap.rs: word wrap shared by prompt and chat panels ────────
//
// Layout a string into terminal rows: break at explicit newlines first,
// then at the last whitespace that fits; a word longer than a row is
// hard-split. Whitespace at a soft wrap boundary is consumed — the next
// row starts after it, so wrapped rows have no ragged leading spaces.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

/// One visual terminal row of the wrapped text.
#[derive(Debug)]
pub struct WrappedRow {
    pub byte_range: (usize, usize),
    pub width: u16,
}

/// Soft-wrap a flat span list to `width` display columns, preserving each
/// run's style across row breaks (Grok prose in the transcript).
pub fn wrap_spans(spans: Vec<Span<'static>>, width: u16) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![Line::from(spans)];
    }
    let full: String = spans.iter().map(|s| s.content.as_ref()).collect();
    if full.is_empty() {
        return vec![Line::from("")];
    }
    // Avoid the trailing empty cursor row that wrap_text adds for empty/newline.
    let rows = wrap_text(&full, width);
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let (start, end) = row.byte_range;
        if start >= end && !full.is_empty() {
            // Skip the synthetic trailing empty row from wrap_text.
            if start == full.len() && end == full.len() && full.ends_with('\n') {
                out.push(Line::from(""));
            }
            continue;
        }
        if start >= end {
            out.push(Line::from(""));
            continue;
        }
        out.push(Line::from(slice_spans(&spans, start, end)));
    }
    if out.is_empty() {
        out.push(Line::from(""));
    }
    out
}

/// Soft-wrap plain text; each visual row becomes one styled line.
pub fn wrap_plain(text: &str, width: u16, style: Style) -> Vec<Line<'static>> {
    if text.is_empty() {
        return vec![Line::from(Span::styled(String::new(), style))];
    }
    let rows = wrap_text(text, width.max(1));
    let mut out = Vec::new();
    for row in rows {
        let (start, end) = row.byte_range;
        if start >= end {
            if start == text.len() && text.ends_with('\n') {
                out.push(Line::from(Span::styled(String::new(), style)));
            }
            continue;
        }
        let slice = text.get(start..end).unwrap_or("").to_string();
        out.push(Line::from(Span::styled(slice, style)));
    }
    if out.is_empty() {
        out.push(Line::from(Span::styled(String::new(), style)));
    }
    out
}

fn slice_spans(spans: &[Span<'static>], start: usize, end: usize) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    for span in spans {
        let len = span.content.len();
        let span_end = pos + len;
        if span_end <= start {
            pos = span_end;
            continue;
        }
        if pos >= end {
            break;
        }
        let from = start.saturating_sub(pos);
        let to = (end - pos).min(len);
        if from < to {
            let text = span.content.as_ref().get(from..to).unwrap_or("").to_string();
            if !text.is_empty() {
                out.push(Span::styled(text, span.style));
            }
        }
        pos = span_end;
    }
    if out.is_empty() {
        out.push(Span::raw(""));
    }
    out
}

fn display_width(s: &str) -> usize {
    s.chars()
        .map(|c| c.width().unwrap_or(0).max(1))
        .sum()
}

pub fn wrap_text(buf: &str, width: u16) -> Vec<WrappedRow> {
    let width = width.max(1) as usize;
    let mut rows = Vec::new();

    for logical in buf.split('\n') {
        let base = logical.as_ptr() as usize - buf.as_ptr() as usize;
        let mut start = 0usize;
        let mut col = 0usize;
        // Last whitespace in the *current* row: (byte offset in `logical`, display col before it).
        let mut last_ws: Option<(usize, usize)> = None;

        for (off, ch) in logical.char_indices() {
            let w = ch.width().unwrap_or(0).max(1);

            // Break until `ch` fits on the current row (or alone when w > width).
            while col + w > width {
                if col == 0 {
                    // Single glyph wider than the row (CJK on 1-col): emit it alone.
                    let ch_end = off + ch.len_utf8();
                    rows.push(WrappedRow {
                        byte_range: (base + off, base + ch_end),
                        width: w.min(width) as u16,
                    });
                    start = ch_end;
                    col = 0;
                    last_ws = None;
                    // `ch` consumed as its own row — skip the normal append below.
                    break;
                }
                match last_ws {
                    Some((ws_off, _ws_col)) if ws_off >= start => {
                        let row_w = display_width(&logical[start..ws_off]);
                        rows.push(WrappedRow {
                            byte_range: (base + start, base + ws_off),
                            width: row_w as u16,
                        });
                        let ws_len = logical[ws_off..]
                            .chars()
                            .next()
                            .map(|c| c.len_utf8())
                            .unwrap_or(1);
                        start = ws_off + ws_len;
                        col = display_width(&logical[start..off]);
                        last_ws = None;
                    }
                    _ => {
                        rows.push(WrappedRow {
                            byte_range: (base + start, base + off),
                            width: col as u16,
                        });
                        start = off;
                        col = 0;
                        last_ws = None;
                    }
                }
            }

            // Wide glyph already emitted as its own row.
            if start > off {
                continue;
            }

            if ch.is_whitespace() {
                last_ws = Some((off, col));
            }
            col += w;
        }

        rows.push(WrappedRow {
            byte_range: (base + start, base + logical.len()),
            width: display_width(&logical[start..]) as u16,
        });
    }
    // A trailing '\n' ends the buffer with a blank row the cursor can sit on.
    if buf.ends_with('\n') || buf.is_empty() {
        rows.push(WrappedRow {
            byte_range: (buf.len(), buf.len()),
            width: 0,
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Modifier};

    #[test]
    fn wrap_spans_preserves_style_across_rows() {
        let bold = Style::default()
            .fg(Color::Red)
            .add_modifier(Modifier::BOLD);
        let spans = vec![
            Span::styled("hello ".to_string(), bold),
            Span::styled("world and friends".to_string(), Style::default()),
        ];
        let lines = wrap_spans(spans, 10);
        assert!(lines.len() >= 2, "expected soft-wrap, got {}", lines.len());
        // First row should still carry the bold "hello" style.
        let first_has_bold = lines[0]
            .spans
            .iter()
            .any(|s| s.style.add_modifier.contains(Modifier::BOLD));
        assert!(first_has_bold);
    }

    #[test]
    fn wrap_plain_breaks_long_words() {
        let lines = wrap_plain("abcdefghij", 4, Style::default());
        assert!(lines.len() >= 2);
    }
}

#[cfg(test)]
mod overflow_props {
    use super::*;
    use unicode_width::UnicodeWidthChar;

    fn row_display_width(buf: &str, row: &WrappedRow) -> usize {
        buf[row.byte_range.0..row.byte_range.1]
            .chars()
            .map(|c| c.width().unwrap_or(0).max(1))
            .sum()
    }

    #[test]
    fn no_row_exceeds_width_for_randomish_inputs() {
        let samples = [
            "a".repeat(500),
            "word ".repeat(200),
            "şğüiöç ".repeat(100),
            "漢字かな ".repeat(80),
            format!("{}\n{}", "x".repeat(200), "y".repeat(200)),
            "a  b   c    d".repeat(50),
            "\t".repeat(20) + &"z".repeat(100),
            "endwithspace ".repeat(30),
            "  leadspace".to_string() + &"m".repeat(100),
        ];
        for width in [1u16, 2, 3, 5, 8, 10, 20, 40, 80] {
            for s in &samples {
                let rows = wrap_text(s, width);
                for (i, r) in rows.iter().enumerate() {
                    let w = row_display_width(s, r);
                    let slice = &s[r.byte_range.0..r.byte_range.1];
                    // A single glyph may be wider than the row (CJK on 1-col);
                    // every other row must fit.
                    let single_wide = slice.chars().count() == 1
                        && slice
                            .chars()
                            .next()
                            .map(|c| c.width().unwrap_or(0).max(1))
                            .unwrap_or(0)
                            > width as usize;
                    assert!(
                        w <= width as usize || single_wide,
                        "width={width} row={i} display={w} slice={slice:?}"
                    );
                }
            }
        }
    }
}
