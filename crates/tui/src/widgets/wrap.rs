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

pub fn wrap_text(buf: &str, width: u16) -> Vec<WrappedRow> {
    let width = width.max(1) as usize;
    let mut rows = Vec::new();

    for logical in buf.split('\n') {
        let base = logical.as_ptr() as usize - buf.as_ptr() as usize;
        let mut start = 0usize;
        let mut col = 0usize;
        let mut last_ws: Option<(usize, usize)> = None;

        for (off, ch) in logical.char_indices() {
            let w = ch.width().unwrap_or(0).max(1);
            if col + w > width && col > 0 {
                match last_ws {
                    Some((ws_off, ws_col)) => {
                        rows.push(WrappedRow {
                            byte_range: (base + start, base + ws_off),
                            width: ws_col as u16,
                        });
                        start = ws_off + logical[ws_off..].chars().next().unwrap().len_utf8();
                        col = col.saturating_sub(ws_col + 1);
                        last_ws = None;
                    }
                    None => {
                        rows.push(WrappedRow {
                            byte_range: (base + start, base + off),
                            width: col as u16,
                        });
                        start = off;
                        col = 0;
                    }
                }
            }
            if ch.is_whitespace() {
                last_ws = Some((off, col));
            }
            col += w;
        }
        rows.push(WrappedRow {
            byte_range: (base + start, base + logical.len()),
            width: col as u16,
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
