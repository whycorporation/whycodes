//! Put text on the system clipboard from the alternate-screen TUI.
//!
//! Terminal mouse selection copies every grid cell — including the spaces we
//! use for backgrounds — so we own selection and push a cleaned string here.
//!
//! Selection shape is **linear** (Grok / native terminal semantics), not a
//! rectangle: first line from the anchor to the end, middle lines full width,
//! last line from the start to the head. Per-line trailing pad is stripped,
//! multi-line selections lose their common left indent (safe-area / SIDE_PAD /
//! assistant gutter), and empty pad-only rows are dropped.
//!
//! OSC 52 works in Kitty/WezTerm/iTerm/Alacritty/Windows Terminal; on Linux we
//! also try `wl-copy` / `xclip` when available.

use std::io::{self, Write};
use std::process::{Command, Stdio};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

/// Copy `text` to the clipboard. Returns true if at least one path succeeded.
pub fn copy_text(text: &str) -> bool {
    let osc = osc52(text);
    let mut ok = write_osc52(&osc);
    ok |= try_wl_copy(text);
    ok |= try_xclip(text);
    ok |= try_pbcopy(text);
    ok
}

fn osc52(text: &str) -> String {
    let b64 = STANDARD.encode(text.as_bytes());
    // `c` = CLIPBOARD, `p` = PRIMARY (middle-click on X11).
    format!("\x1b]52;c;{b64}\x07\x1b]52;p;{b64}\x07")
}

fn write_osc52(seq: &str) -> bool {
    let mut out = io::stdout().lock();
    out.write_all(seq.as_bytes()).is_ok() && out.flush().is_ok()
}

fn try_wl_copy(text: &str) -> bool {
    pipe_to(&["wl-copy"], text)
}

fn try_xclip(text: &str) -> bool {
    pipe_to(&["xclip", "-selection", "clipboard"], text)
        || pipe_to(&["xclip", "-selection", "primary"], text)
}

fn try_pbcopy(text: &str) -> bool {
    pipe_to(&["pbcopy"], text)
}

fn pipe_to(cmd: &[&str], text: &str) -> bool {
    let (bin, args) = match cmd.split_first() {
        Some((b, a)) => (*b, a),
        None => return false,
    };
    let mut child = match Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let ok = child
        .stdin
        .as_mut()
        .and_then(|stdin| stdin.write_all(text.as_bytes()).ok())
        .is_some();
    let status = child.wait().map(|s| s.success()).unwrap_or(false);
    ok && status
}

// ── Linear selection geometry ──────────────────────────────────────────

/// Inclusive column range for one screen row under a linear drag.
///
/// Callers must pass reading-order endpoints: `y0 <= y1`, and on a single row
/// `x0`/`x1` are the left/right columns (either order is fine).
/// `row_max_x` is the last valid column index of the row (width - 1).
pub fn linear_cols(
    y: u16,
    y0: u16,
    y1: u16,
    x0: u16,
    x1: u16,
    row_max_x: u16,
) -> Option<(u16, u16)> {
    debug_assert!(y0 <= y1);
    if y < y0 || y > y1 {
        return None;
    }
    if y0 == y1 {
        let lo = x0.min(x1).min(row_max_x);
        let hi = x0.max(x1).min(row_max_x);
        return Some((lo, hi));
    }
    if y == y0 {
        // First line of a multi-line drag: from click to end of row.
        return Some((x0.min(row_max_x), row_max_x));
    }
    if y == y1 {
        // Last line: from start of row to release column.
        return Some((0, x1.min(row_max_x)));
    }
    // Middle lines: full width (trailing pad stripped later).
    Some((0, row_max_x))
}

/// True when a cell is visual padding (background fill / wide-glyph tail).
pub fn is_pad_symbol(sym: &str) -> bool {
    sym.is_empty() || sym == " " || sym == "\u{00a0}" || sym == "\t"
}

/// Shrink `(xs, xe)` so the range ends on the last non-pad cell.
/// Returns `None` when the slice is pad-only (nothing to paint or copy).
pub fn content_end(row: &[String], xs: usize, xe: usize) -> Option<usize> {
    content_span(row, xs, xe).map(|(_, end)| end)
}

/// First and last non-pad indices inside `[xs, xe]`.
///
/// Used for the selection overlay so leading layout pad and trailing
/// background fill are not reverse-video'd. Clipboard extraction still
/// starts at the raw linear `xs` and dedents common indent afterward.
pub fn content_span(row: &[String], xs: usize, xe: usize) -> Option<(usize, usize)> {
    if row.is_empty() || xs > xe {
        return None;
    }
    let xe = xe.min(row.len().saturating_sub(1));
    let xs = xs.min(xe);
    let start = (xs..=xe).find(|&i| row.get(i).is_some_and(|s| !is_pad_symbol(s)))?;
    let end = (start..=xe)
        .rev()
        .find(|&i| row.get(i).is_some_and(|s| !is_pad_symbol(s)))?;
    Some((start, end))
}

/// Build clipboard text from a **linear** selection over a cell grid.
///
/// This is the Grok-like path: multi-line drags do not pull in the empty
/// rectangle corners to the right of short lines, trailing pad is stripped,
/// shared left indent (layout chrome) is dedented, and pad-only rows drop out.
pub fn text_from_cells(cells: &[Vec<String>], x0: u16, y0: u16, x1: u16, y1: u16) -> String {
    text_from_cells_linear(cells, x0, y0, x1, y1)
}

pub fn text_from_cells_linear(cells: &[Vec<String>], x0: u16, y0: u16, x1: u16, y1: u16) -> String {
    if cells.is_empty() {
        return String::new();
    }

    // Reading-order endpoints: top-to-bottom, and on a single row left-to-right.
    let (top_y, bot_y, top_x, bot_x) = reading_order(x0, y0, x1, y1);

    let mut lines: Vec<String> = Vec::new();
    for y in top_y..=bot_y {
        let Some(row) = cells.get(y as usize) else {
            continue;
        };
        if row.is_empty() {
            continue;
        }
        let row_max = (row.len().saturating_sub(1)) as u16;
        let Some((xs, xe)) = linear_cols(y, top_y, bot_y, top_x, bot_x, row_max) else {
            continue;
        };
        let xs = xs as usize;
        let xe = xe as usize;
        let Some(end) = content_end(row, xs, xe) else {
            // Pad-only row inside the drag — keep a blank line only when it
            // sits between real content (collapsed later).
            lines.push(String::new());
            continue;
        };
        lines.push(extract_row(row, xs, end));
    }

    clean_copied_lines(lines).join("\n")
}

/// Post-process extracted screen lines into paste-friendly text.
///
/// Screen cells include layout chrome the user never wants:
/// - trailing background fill (already stripped in extract)
/// - **space-between** footer gaps (`path` + 80 spaces + `status`)
/// - shared left inset (SIDE_PAD / safe area)
/// - empty message-gap rows
fn clean_copied_lines(mut lines: Vec<String>) -> Vec<String> {
    // Drop leading/trailing blank rows.
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    while lines.first().is_some_and(|l| l.is_empty()) {
        lines.remove(0);
    }
    // Collapse runs of blank lines to a single blank (message gaps).
    lines = collapse_blank_runs(lines);
    // Kill space-between layout: after the first non-space, any run of
    // spaces becomes a single space. Leading indent is preserved so
    // dedent can still recover relative code indent.
    lines = lines
        .into_iter()
        .map(|l| collapse_interior_spaces(&l))
        .collect();
    // Multi-line: strip shared left padding (safe inset, SIDE_PAD, gutters).
    // Also run on a single line so a lone status/prompt row loses its inset.
    lines = dedent_common(lines);
    // One more trim_end after collapse (paranoia).
    lines.into_iter().map(|l| trim_end_pad(&l)).collect()
}

/// Order a drag so `top` is the earlier reading-order endpoint.
fn reading_order(x0: u16, y0: u16, x1: u16, y1: u16) -> (u16, u16, u16, u16) {
    if y0 < y1 {
        (y0, y1, x0, x1)
    } else if y1 < y0 {
        (y1, y0, x1, x0)
    } else if x0 <= x1 {
        (y0, y1, x0, x1)
    } else {
        (y0, y1, x1, x0)
    }
}

fn extract_row(row: &[String], xs: usize, end: usize) -> String {
    let mut s = String::new();
    let mut x = xs;
    while x <= end {
        let sym = row.get(x).map(|s| s.as_str()).unwrap_or("");
        // Multi-width glyphs occupy one logical cell + blank continuations
        // in the ratatui buffer; advance by display width so we don't
        // double-append empty continuation cells as spaces.
        let w = unicode_width::UnicodeWidthStr::width(sym).max(1);
        if !sym.is_empty() {
            s.push_str(sym);
        }
        x += w;
    }
    trim_end_pad(&s)
}

fn trim_end_pad(s: &str) -> String {
    s.trim_end_matches([' ', '\t', '\u{00a0}']).to_string()
}

fn collapse_blank_runs(lines: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(lines.len());
    let mut prev_blank = false;
    for l in lines {
        let blank = l.is_empty();
        if blank && prev_blank {
            continue;
        }
        out.push(l);
        prev_blank = blank;
    }
    out
}

/// Collapse layout filler spaces without touching leading indent.
///
/// Footer/status rows paint `left + " ".repeat(gap) + right` for
/// space-between alignment. Those gaps are real space cells and used to
/// land in the clipboard as dozens of spaces. After the first non-space
/// character, any run of whitespace becomes a single ASCII space.
///
/// Leading spaces/tabs are kept so [`dedent_common`] can still strip the
/// shared inset while preserving relative code indent on later columns.
fn collapse_interior_spaces(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut seen_content = false;
    let mut pending_ws = 0usize;
    for c in s.chars() {
        if c == ' ' || c == '\t' || c == '\u{00a0}' {
            if !seen_content {
                // Leading indent — keep as plain spaces (tabs → space).
                out.push(' ');
            } else {
                pending_ws += 1;
            }
            continue;
        }
        if pending_ws > 0 {
            out.push(' ');
            pending_ws = 0;
        }
        seen_content = true;
        out.push(c);
    }
    // Trailing ws intentionally dropped (matches trim_end).
    out
}

/// Remove the largest run of leading ASCII spaces shared by every non-empty line.
fn dedent_common(lines: Vec<String>) -> Vec<String> {
    let min_lead = lines
        .iter()
        .filter(|l| !l.is_empty())
        .map(|l| l.chars().take_while(|c| *c == ' ').count())
        .min()
        .unwrap_or(0);
    if min_lead == 0 {
        return lines;
    }
    lines
        .into_iter()
        .map(|l| {
            if l.is_empty() {
                l
            } else {
                l.chars().skip(min_lead).collect()
            }
        })
        .collect()
}

/// Inclusive linear ranges to paint for a drag, content-clamped per row.
///
/// Used by the selection overlay so empty pad cells to the right of short
/// lines are not reverse-video'd (matches what ends up on the clipboard).
pub fn paint_ranges(
    cells: &[Vec<String>],
    x0: u16,
    y0: u16,
    x1: u16,
    y1: u16,
) -> Vec<(u16, u16, u16)> {
    // (y, x_start, x_end) inclusive
    let mut out = Vec::new();
    if cells.is_empty() {
        return out;
    }
    let (top_y, bot_y, top_x, bot_x) = reading_order(x0, y0, x1, y1);
    for y in top_y..=bot_y {
        let Some(row) = cells.get(y as usize) else {
            continue;
        };
        if row.is_empty() {
            continue;
        }
        let row_max = (row.len().saturating_sub(1)) as u16;
        let Some((xs, xe)) = linear_cols(y, top_y, bot_y, top_x, bot_x, row_max) else {
            continue;
        };
        let Some((start, end)) = content_span(row, xs as usize, xe as usize) else {
            continue;
        };
        out.push((y, start as u16, end as u16));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(rows: &[&str]) -> Vec<Vec<String>> {
        rows.iter()
            .map(|r| r.chars().map(|c| c.to_string()).collect())
            .collect()
    }

    /// Pad every row to `width` with spaces (screen-like background fill).
    fn grid_padded(rows: &[&str], width: usize) -> Vec<Vec<String>> {
        rows.iter()
            .map(|r| {
                let mut cells: Vec<String> = r.chars().map(|c| c.to_string()).collect();
                while cells.len() < width {
                    cells.push(" ".into());
                }
                cells
            })
            .collect()
    }

    #[test]
    fn trims_trailing_spaces_per_line() {
        let cells = grid_padded(&["hi", "yo", ""], 8);
        let t = text_from_cells(&cells, 0, 0, 7, 2);
        assert_eq!(t, "hi\nyo");
    }

    #[test]
    fn keeps_internal_spaces() {
        let cells = grid_padded(&["a b c"], 10);
        let t = text_from_cells(&cells, 0, 0, 9, 0);
        assert_eq!(t, "a b c");
    }

    #[test]
    fn sub_rect_same_row() {
        let cells = grid(&["abcdefgh", "ijklmnop"]);
        let t = text_from_cells(&cells, 2, 0, 4, 0);
        assert_eq!(t, "cde");
    }

    #[test]
    fn linear_multi_line_drops_rectangle_corners() {
        // Screen: short lines padded to width 10 with spaces.
        // Drag from (0,0)='a' down to (2,1)='l' — a rectangle would also
        // include spaces after "hello" on line 0; linear + trim must not.
        let cells = grid_padded(&["hello", "world"], 10);
        // top-left 'h' (0,0) → bottom at col 2 'r' of "world"
        let t = text_from_cells(&cells, 0, 0, 2, 1);
        assert_eq!(t, "hello\nwor");
    }

    #[test]
    fn linear_first_line_starts_mid_row() {
        let cells = grid_padded(&["hello world", "second line"], 14);
        // Start at 'w' of hello world (col 6), end mid second line.
        let t = text_from_cells(&cells, 6, 0, 5, 1);
        assert_eq!(t, "world\nsecond");
    }

    #[test]
    fn dedents_shared_left_padding() {
        // Common 2-space layout pad + content.
        let cells = grid_padded(&["  foo", "  bar", "  baz"], 10);
        let t = text_from_cells(&cells, 0, 0, 9, 2);
        assert_eq!(t, "foo\nbar\nbaz");
    }

    #[test]
    fn preserves_relative_indent_after_dedent() {
        let cells = grid_padded(&["  if x:", "    return", "  end"], 12);
        let t = text_from_cells(&cells, 0, 0, 11, 2);
        assert_eq!(t, "if x:\n  return\nend");
    }

    #[test]
    fn paint_ranges_skip_trailing_and_leading_pad() {
        let cells = grid_padded(&["  hi", "  yo"], 8);
        let r = paint_ranges(&cells, 0, 0, 7, 1);
        // Content span skips the two leading spaces and trailing pad.
        assert_eq!(r, vec![(0, 2, 3), (1, 2, 3)]);
    }

    #[test]
    fn paint_ranges_same_row_partial() {
        let cells = grid_padded(&["abcdef"], 10);
        let r = paint_ranges(&cells, 1, 0, 3, 0);
        assert_eq!(r, vec![(0, 1, 3)]);
    }

    #[test]
    fn empty_selection_on_pad_only_is_empty() {
        let cells = grid_padded(&["", "  "], 6);
        let t = text_from_cells(&cells, 0, 0, 5, 1);
        assert_eq!(t, "");
    }

    #[test]
    fn wide_glyph_not_doubled() {
        // '世' is width 2; ratatui stores symbol at col0 and empty at col1.
        let mut row: Vec<String> = vec!["世".into(), "".into(), "a".into(), " ".into()];
        while row.len() < 6 {
            row.push(" ".into());
        }
        let cells = vec![row];
        let t = text_from_cells(&cells, 0, 0, 5, 0);
        assert_eq!(t, "世a");
    }

    #[test]
    fn status_bar_space_between_collapses() {
        // Footer: "● whycode" + many pad spaces + "Get started /connect"
        let mut row = String::from("● whycode");
        row.push_str(&" ".repeat(40));
        row.push_str("Get started /connect");
        let cells = grid_padded(&[row.as_str()], 80);
        let t = text_from_cells(&cells, 0, 0, 79, 0);
        assert_eq!(t, "● whycode Get started /connect");
        assert!(
            !t.contains("  "),
            "must not keep double spaces from layout: {t:?}"
        );
    }

    #[test]
    fn interior_double_space_becomes_one() {
        let cells = grid_padded(&["hello  world"], 20);
        let t = text_from_cells(&cells, 0, 0, 19, 0);
        assert_eq!(t, "hello world");
    }

    #[test]
    fn relative_indent_survives_after_dedent_and_collapse() {
        // Shared 2-space inset + real nested indent.
        let cells = grid_padded(&["  if x:", "    return 1", "  end"], 16);
        let t = text_from_cells(&cells, 0, 0, 15, 2);
        assert_eq!(t, "if x:\n  return 1\nend");
    }

    #[test]
    fn clean_copied_lines_collapses_status_and_dedents() {
        // Shared inset is 1 space (status row only has one); remaining
        // two spaces on chat rows stay. Status gap of 30 spaces → one.
        let lines = vec![
            "   ┃ hello".into(),
            "   │ note".into(),
            format!(" ● whycode{}Get started", " ".repeat(30)),
        ];
        let out = clean_copied_lines(lines);
        assert_eq!(
            out,
            vec![
                "  ┃ hello".to_string(),
                "  │ note".to_string(),
                "● whycode Get started".to_string(),
            ]
        );
        assert!(
            !out[2].contains("  "),
            "status gap must collapse: {:?}",
            out[2]
        );
    }

    #[test]
    fn uniform_inset_fully_stripped() {
        let lines = vec!["   ┃ hello".into(), "   │ note".into(), "   ┃ more".into()];
        let out = clean_copied_lines(lines);
        assert_eq!(out, vec!["┃ hello", "│ note", "┃ more"]);
    }
}
