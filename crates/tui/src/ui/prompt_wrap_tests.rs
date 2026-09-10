use super::*;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

fn row_texts(buf: &str, width: u16) -> Vec<String> {
    wrap_text(buf, width)
        .iter()
        .map(|r| buf[r.byte_range.0..r.byte_range.1].to_string())
        .collect()
}

#[test]
fn short_text_stays_on_one_row() {
    assert_eq!(row_texts("hello", 20), vec!["hello"]);
}

#[test]
fn wraps_at_word_boundaries() {
    // "bbb ccc" fits on one 7-col row, so the only break is after "aaa".
    assert_eq!(row_texts("aaa bbb ccc", 7), vec!["aaa", "bbb ccc"]);
    // Narrower: each word gets its own row.
    assert_eq!(row_texts("aaa bbb ccc", 4), vec!["aaa", "bbb", "ccc"]);
}

#[test]
fn long_word_is_hard_split() {
    let rows = row_texts("abcdefghij", 4);
    assert_eq!(rows, vec!["abcd", "efgh", "ij"]);
}

#[test]
fn explicit_newline_breaks_row() {
    let rows = row_texts("ab\ncd", 10);
    assert_eq!(rows, vec!["ab", "cd"]);
}

#[test]
fn multibyte_chars_count_by_display_width() {
    // 'ş' is 1 column; CJK '世' is 2.
    let rows = row_texts("ş世a", 3);
    assert_eq!(rows, vec!["ş世", "a"]);
}

#[test]
fn every_byte_is_covered_exactly_once() {
    let buf = "the quick brown fox jumps over the lazy dog";
    let rows = wrap_text(buf, 10);
    let mut covered = String::new();
    let mut prev_end = 0;
    for r in &rows {
        assert_eq!(r.byte_range.0, prev_end, "rows must be contiguous");
        assert!(r.width as usize <= 10);
        covered.push_str(&buf[r.byte_range.0..r.byte_range.1]);
        prev_end = r.byte_range.1;
        // Skip the single whitespace consumed by the wrap boundary.
        if prev_end < buf.len()
            && buf.as_bytes()[prev_end].is_ascii_whitespace()
            && buf.as_bytes()[prev_end] != b'\n'
        {
            covered.push(buf.as_bytes()[prev_end] as char);
            prev_end += 1;
        }
    }
    assert_eq!(covered, buf);
}

#[test]
fn prompt_height_includes_box_chrome() {
    // gap + top + 1 text + bottom + hint
    const {
        assert!(OUTER_TOP_GAP + VPAD_TOP + 1 + INFO_BLOCK + HINT_GAP >= 5);
    }
}

#[test]
fn center_prompt_uses_almost_full_width_on_phone() {
    let area = Rect::new(0, 0, 40, 6);
    let boxed = center_prompt_area(area);
    assert!(
        boxed.width >= 36,
        "portrait prompt.width={} should not sit at the old 40-col floor",
        boxed.width
    );
    assert!(boxed.x + boxed.width <= area.width);
}

#[test]
fn slash_command_byte_end_covers_token_only() {
    assert_eq!(slash_command_byte_end("/help"), Some(5));
    assert_eq!(slash_command_byte_end("/models foo"), Some(7));
    assert_eq!(slash_command_byte_end("/"), Some(1));
    assert_eq!(slash_command_byte_end("hello"), None);
    assert_eq!(slash_command_byte_end(""), None);
}

#[test]
fn styled_input_row_splits_command_and_args() {
    use ratatui::style::{Color, Modifier};
    let cmd = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let text = Style::default().fg(Color::White);
    let paste = Style::default().fg(Color::Yellow);
    let buf = "/help me";
    let spans = styled_input_row(
        buf,
        0,
        buf.len(),
        slash_command_byte_end(buf),
        cmd,
        text,
        paste,
        &[],
    );
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].content.as_ref(), "/help");
    assert_eq!(spans[1].content.as_ref(), " me");
    assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn styled_input_row_highlights_paste_token() {
    use ratatui::style::Color;
    let cmd = Style::default();
    let text = Style::default().fg(Color::White);
    let paste = Style::default().fg(Color::Yellow);
    let token = crate::paste::placeholder(1, 5);
    let buf = format!("see {token} ok");
    let ranges = crate::paste::style_ranges(&buf);
    let spans = styled_input_row(&buf, 0, buf.len(), None, cmd, text, paste, &ranges);
    assert!(
        spans.iter().any(|s| s.content.as_ref() == token),
        "paste token should be its own span: {spans:?}"
    );
    let token_span = spans.iter().find(|s| s.content.as_ref() == token).unwrap();
    assert_eq!(token_span.style.fg, Some(Color::Yellow));
}

#[test]
fn truncate_pick_hint_and_cursor_helpers() {
    assert!(truncate_to_width("hello", 0).is_empty());
    assert_eq!(truncate_to_width("hello", 3), "hel");
    let _ = pick_hint();
    assert_eq!(HINTS.len(), 9);
    for hint in HINTS {
        assert!(!hint.is_empty(), "rotating home hints must be non-empty");
    }
    use ratatui::style::{Color, Style};
    assert!(
        styled_input_row(
            "abc",
            2,
            2,
            None,
            Style::default(),
            Style::default(),
            Style::default().fg(Color::Yellow),
            &[],
        )
        .is_empty()
    );
    assert_eq!(cursor_row_col(&[], "hi", 0), (0, 0));
    let rows = wrap_text("hello world", 5);
    let (row, col) = cursor_row_col(&rows, "hello world", 0);
    assert_eq!(row, 0);
    assert_eq!(col, 0);

    let mut app = crate::app::TuiApp::new(crate::config::TuiAppConfig::default());
    assert_eq!(attach_row_count(&app), 0);
    app.pending_images.push(crate::images::PromptImage {
        path: "a.png".into(),
        label: "a.png".into(),
        media_type: "image/png".into(),
    });
    assert_eq!(attach_row_count(&app), 1);
    assert!(prompt_height(&app, 80) > 1);
    assert!(content_width(&app, 80) >= 8);
    assert!(prompt_owns_caret(&app));
    app.focus = crate::app::FocusPane::Scrollback;
    assert!(!prompt_owns_caret(&app));

    let backend = TestBackend::new(8, 4);
    let mut terminal = Terminal::new(backend).expect("term");
    let style = Style::default();
    terminal
        .draw(|f| {
            paint_h_border(f, Rect::new(0, 0, 0, 1), style, true);
            paint_h_border(f, Rect::new(0, 1, 1, 1), style, true);
            paint_h_border(f, Rect::new(0, 2, 1, 1), style, false);
            paint_h_border(f, Rect::new(2, 0, 4, 1), style, true);
        })
        .expect("draw");
    let buf = terminal.backend().buffer();
    assert_eq!(buf[(0, 1)].symbol(), "╭");
    assert_eq!(buf[(0, 2)].symbol(), "╰");
}

#[test]
fn clamp_rows_to_width_keeps_empty_and_cuts_overwide() {
    let buf = "abcdefghij";
    let empty = crate::widgets::wrap::WrappedRow {
        byte_range: (4, 4),
        width: 0,
    };
    let kept = clamp_rows_to_width(buf, vec![empty], 8);
    assert_eq!(kept[0].byte_range, (4, 4));

    let wide = crate::widgets::wrap::WrappedRow {
        byte_range: (0, buf.len()),
        width: 10,
    };
    let cut = clamp_rows_to_width(buf, vec![wide], 4);
    assert_eq!(cut[0].byte_range.0, 0);
    assert!(
        cut[0].byte_range.1 < buf.len(),
        "over-wide wrap must cut before the end, got {:?}",
        cut[0].byte_range
    );
    assert!(cut[0].width as usize <= 4);
}
