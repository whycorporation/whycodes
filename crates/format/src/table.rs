//! ASCII / box-drawing table layout for markdown and tool display.

use crate::markdown::TableAlign;

/// Format data as an ASCII table with pipe separators and auto-sized columns.
///
/// Returns the formatted table as a String. Columns are left-aligned.
pub fn format_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let aligns = vec![TableAlign::Left; headers.len()];
    format_table_aligned(headers, rows, &aligns)
}

/// Like [`format_table`], with per-column GFM alignment.
pub fn format_table_aligned(
    headers: &[&str],
    rows: &[Vec<String>],
    aligns: &[TableAlign],
) -> String {
    format_table_lines(headers, rows, aligns).join("\n")
}

/// Line-oriented table (no trailing newline). Used by ANSI markdown and tests.
pub fn format_table_lines(
    headers: &[&str],
    rows: &[Vec<String>],
    aligns: &[TableAlign],
) -> Vec<String> {
    if headers.is_empty() {
        return Vec::new();
    }

    let col_count = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| display_width(h)).collect();

    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < col_count {
                widths[i] = widths[i].max(display_width(cell));
            }
        }
    }

    let mut out = Vec::with_capacity(rows.len() + 4);
    out.push(format_border(&widths, BorderKind::Top));
    out.push(format_row(headers, &widths, aligns, col_count));
    out.push(format_border(&widths, BorderKind::Mid));
    for row in rows {
        let cells: Vec<&str> = (0..col_count)
            .map(|i| row.get(i).map(|s| s.as_str()).unwrap_or(""))
            .collect();
        out.push(format_row(&cells, &widths, aligns, col_count));
    }
    out.push(format_border(&widths, BorderKind::Bot));
    out
}

/// Column widths (chars) for headers + rows, optionally capped so the table
/// fits `max_total` display columns (including box chrome).
///
/// Returns `(widths, chrome_width)` where chrome is the fixed `│ ` / ` │`
/// overhead for all columns (`3 * cols + 1` for outer pipes + pads).
pub fn column_widths(
    headers: &[String],
    rows: &[Vec<String>],
    max_total: Option<usize>,
) -> Vec<usize> {
    let col_count = headers.len();
    if col_count == 0 {
        return Vec::new();
    }
    let mut widths: Vec<usize> = headers.iter().map(|h| display_width(h)).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < col_count {
                widths[i] = widths[i].max(display_width(cell));
            }
        }
    }
    // Ensure at least 1 col width so borders don't collapse.
    for w in &mut widths {
        *w = (*w).max(1);
    }

    if let Some(max_total) = max_total {
        // Chrome: left │ + each col (space + cell + space + │) = 1 + cols*3 + sum(widths)
        // Actually: `│ cell │ cell │` = 1 + sum(2 + w) for each col...
        // format: `| padded |` with 1 space pad each side → per col: 1 + w + 1 + 1(pipe) but first pipe shared.
        // Total = 1 + sum_i (w_i + 3)  wait: "| a | b |" = 1 + (1+w+1+1)*n ... simpler:
        // border parts: each col contributes w+2 dashes between +; row: `| ` + cell + ` |` * n
        // width = 1 + sum(w_i + 3) - something. Use: 1 + n + sum(w_i + 2) = 1 + 3n + sum(w)
        let chrome = 1 + 3 * col_count;
        let max_cells = max_total.saturating_sub(chrome).max(col_count);
        let total: usize = widths.iter().sum();
        if total > max_cells {
            // Shrink widest columns first until we fit.
            let mut excess = total - max_cells;
            while excess > 0 && widths.iter().any(|w| *w > 1) {
                if let Some((idx, _)) = widths
                    .iter()
                    .enumerate()
                    .filter(|(_, w)| **w > 1)
                    .max_by_key(|(_, w)| *w)
                {
                    widths[idx] -= 1;
                    excess -= 1;
                }
            }
        }
    }
    widths
}

/// Display width in terminal cells (ASCII + common Latin-1 as 1; CJK-ish as 2).
pub fn display_width(s: &str) -> usize {
    s.chars()
        .map(|c| {
            // Fullwidth / wide CJK block heuristic without depending on unicode-width.
            match c {
                '\u{1100}'..='\u{115F}'
                | '\u{2E80}'..='\u{A4CF}'
                | '\u{AC00}'..='\u{D7A3}'
                | '\u{F900}'..='\u{FAFF}'
                | '\u{FE10}'..='\u{FE19}'
                | '\u{FE30}'..='\u{FE6F}'
                | '\u{FF00}'..='\u{FF60}'
                | '\u{FFE0}'..='\u{FFE6}'
                | '\u{1F300}'..='\u{1F9FF}' => 2,
                _ => 1,
            }
        })
        .sum()
}

/// Pad / truncate a cell to `width` display columns with alignment.
pub fn pad_cell(cell: &str, width: usize, align: TableAlign) -> String {
    let w = display_width(cell);
    if w > width {
        return truncate_to_width(cell, width);
    }
    let pad = width - w;
    match align {
        TableAlign::Left => format!("{cell}{}", " ".repeat(pad)),
        TableAlign::Right => format!("{}{cell}", " ".repeat(pad)),
        TableAlign::Center => {
            let left = pad / 2;
            let right = pad - left;
            format!("{}{cell}{}", " ".repeat(left), " ".repeat(right))
        }
    }
}

pub(crate) fn truncate_to_width(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if display_width(s) <= width {
        return s.to_string();
    }
    if width == 1 {
        return "…".to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for c in s.chars() {
        let cw = display_width(&c.to_string());
        if used + cw > width.saturating_sub(1) {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push('…');
    // Pad if ellipsis left us short (wide-char edge cases).
    let final_w = display_width(&out);
    if final_w < width {
        out.push_str(&" ".repeat(width - final_w));
    }
    out
}

enum BorderKind {
    Top,
    Mid,
    Bot,
}

fn format_border(widths: &[usize], kind: BorderKind) -> String {
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
    s
}

fn format_row(cells: &[&str], widths: &[usize], aligns: &[TableAlign], col_count: usize) -> String {
    let mut s = String::from("│");
    for i in 0..col_count {
        let cell = cells.get(i).copied().unwrap_or("");
        let align = aligns.get(i).copied().unwrap_or(TableAlign::Left);
        let w = widths.get(i).copied().unwrap_or(1);
        s.push(' ');
        s.push_str(&pad_cell(cell, w, align));
        s.push(' ');
        s.push('│');
    }
    s
}

#[cfg(test)]
#[path = "table_tests.rs"]
mod tests;
