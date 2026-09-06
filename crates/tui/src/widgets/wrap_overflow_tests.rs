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
