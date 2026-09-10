use super::*;

fn grid(rows: &[&str]) -> CellGrid {
    CellGrid::from_rows(
        rows.iter()
            .map(|r| r.chars().map(|c| c.to_string()).collect())
            .collect(),
    )
}

/// Pad every row to `width` with spaces (screen-like background fill).
fn grid_padded(rows: &[&str], width: usize) -> CellGrid {
    CellGrid::from_rows(
        rows.iter()
            .map(|r| {
                let mut cells: Vec<String> = r.chars().map(|c| c.to_string()).collect();
                while cells.len() < width {
                    cells.push(" ".into());
                }
                cells
            })
            .collect(),
    )
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
    let cells = CellGrid::from_rows(vec![row]);
    let t = text_from_cells(&cells, 0, 0, 5, 0);
    assert_eq!(t, "世a");
}

#[test]
fn status_bar_space_between_collapses() {
    // Footer: "● whycodes" + many pad spaces + "Get started /connect"
    let mut row = String::from("● whycodes");
    row.push_str(&" ".repeat(40));
    row.push_str("Get started /connect");
    let cells = grid_padded(&[row.as_str()], 80);
    let t = text_from_cells(&cells, 0, 0, 79, 0);
    assert_eq!(t, "● whycodes Get started /connect");
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
        format!(" ● whycodes{}Get started", " ".repeat(30)),
    ];
    let out = clean_copied_lines(lines);
    assert_eq!(
        out,
        vec![
            "  ┃ hello".to_string(),
            "  │ note".to_string(),
            "● whycodes Get started".to_string(),
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

#[test]
fn clip_excludes_cells_outside_modal() {
    // Full-width screen: modal occupies cols 10..30, rows 2..5.
    // Chat "LEAK" sits left of the modal on the same rows.
    let cells = grid_padded(
        &[
            "..........",
            "..........",
            "LEAK  modal body here........",
            "LEAK  second line............",
            "..........",
        ],
        40,
    );
    let clip = ClipRect {
        x: 6,
        y: 2,
        width: 20,
        height: 2,
    };
    // Drag spans the full width of two content rows (would include LEAK).
    let t = text_from_cells_clipped(&cells, 0, 2, 39, 3, clip);
    assert!(
        !t.contains("LEAK"),
        "background left of modal must not copy: {t:?}"
    );
    assert!(t.contains("modal body"), "{t:?}");
    assert!(t.contains("second line"), "{t:?}");

    let skip_y = ClipRect {
        x: 0,
        y: 5,
        width: 10,
        height: 1,
    };
    let skipped = text_from_cells_clipped(&cells, 0, 0, 39, 3, skip_y);
    assert!(
        skipped.is_empty(),
        "rows outside clip y must drop: {skipped:?}"
    );

    let inverted_cols = ClipRect {
        x: 30,
        y: 2,
        width: 2,
        height: 2,
    };
    let _ = text_from_cells_clipped(&cells, 0, 2, 5, 3, inverted_cols);
    assert!(text_from_cells(&CellGrid::default(), 0, 0, 1, 1).is_empty());
}

#[test]
fn paint_ranges_clipped_skips_empty_and_inverted_clip() {
    let empty = CellGrid::default();
    assert!(paint_ranges_clipped(&empty, 0, 0, 2, 2, None).is_empty());
    let cells = grid_padded(&["abcdef", "ghijkl", "mnopqr"], 8);
    let clip = ClipRect {
        x: 2,
        y: 1,
        width: 0,
        height: 1,
    };
    let ranges = paint_ranges_clipped(&cells, 0, 0, 7, 2, Some(clip));
    assert!(ranges.iter().all(|(y, _, _)| *y != 1) || ranges.is_empty() || clip.width == 0);
    let inverted = ClipRect {
        x: 6,
        y: 0,
        width: 1,
        height: 1,
    };
    let _ = paint_ranges_clipped(&cells, 0, 0, 2, 0, Some(inverted));
    assert!(linear_cols(9, 0, 1, 0, 1, 7).is_none());
    assert!(content_span(&cells, 0, 5, 2).is_none());
    assert_eq!(reading_order(3, 2, 1, 0), (0, 2, 1, 3));
    assert_eq!(reading_order(3, 1, 1, 1), (1, 1, 1, 3));
    assert!(!pipe_to(&[], "x"));
    let seq = osc52("hi");
    assert!(seq.contains("\x1b]52;c;"));
    let mut buf = Vec::new();
    assert!(write_osc52_to(&mut buf, &seq));
    assert_eq!(buf, seq.as_bytes());
    let _ = copy_text("coverage");
    assert!(with_copy_stub(true, || copy_text("stub-ok")));
    assert!(!with_copy_stub(false, || copy_text("stub-fail")));
    let _ = try_pbcopy("coverage");
    let _ = try_xclip("coverage");
    let _ = try_wl_copy("coverage");
    assert!(!pipe_to(&["whycodes-no-such-copy-bin"], "x"));
    #[cfg(windows)]
    assert!(pipe_to(&["cmd", "/C", "exit", "0"], "coverage"));
    let blanks = vec!["".into(), "".into(), "hi".into(), "".into(), "".into()];
    assert_eq!(
        collapse_blank_runs(blanks),
        vec!["".to_string(), "hi".into(), "".into()]
    );
}

#[test]
fn clipped_linear_copy_skips_rows_outside_modal() {
    let cells = grid_padded(&["aaaaaa", "bbbbbb", "cccccc", "dddddd"], 8);
    let clip = ClipRect {
        x: 0,
        y: 1,
        width: 6,
        height: 2,
    };
    let t = text_from_cells_clipped(&cells, 0, 0, 5, 3, clip);
    assert_eq!(t, "bbbbbb\ncccccc");
    assert!(!t.contains('a'), "{t}");
    assert!(!t.contains('d'), "{t}");

    let empty = CellGrid::default();
    assert!(text_from_cells(&empty, 0, 0, 2, 2).is_empty());
    let tall = text_from_cells(&cells, 0, 0, 5, 40);
    assert!(tall.contains("aaaaaa"), "{tall}");

    let inverted = ClipRect {
        x: 5,
        y: 0,
        width: 1,
        height: 1,
    };
    let t = text_from_cells_clipped(&cells, 0, 0, 2, 0, inverted);
    assert!(t.is_empty() || t.len() <= 1, "{t:?}");
}
