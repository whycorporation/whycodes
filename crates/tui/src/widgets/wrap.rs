// ── widgets/wrap.rs: word wrap shared by prompt and chat panels ────────
//
// Layout a string into terminal rows: break at explicit newlines first,
// then at the last whitespace that fits; a word longer than a row is
// hard-split. Whitespace at a soft wrap boundary is consumed — the next
// row starts after it, so wrapped rows have no ragged leading spaces.

use unicode_width::UnicodeWidthChar;

/// One visual terminal row of the wrapped text.
#[derive(Debug)]
pub struct WrappedRow {
    pub byte_range: (usize, usize),
    pub width: u16,
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
