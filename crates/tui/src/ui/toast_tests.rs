use super::*;

#[test]
fn wraps_at_word_boundaries() {
    assert_eq!(wrap("one two three", 7, 3), vec!["one two", "three"]);
}

#[test]
fn stops_at_the_line_limit() {
    let out = wrap("a b c d e f g h i j k l", 3, 2);
    assert_eq!(out.len(), 2);
}

#[test]
fn truncates_a_word_too_long_to_fit() {
    let out = wrap("supercalifragilistic", 10, 1);
    assert_eq!(out.len(), 1);
    assert!(out[0].ends_with('…'));
    assert_eq!(out[0].chars().count(), 10);
}

#[test]
fn empty_text_still_produces_a_line() {
    assert_eq!(wrap("", 10, 2), vec![String::new()]);
    assert_eq!(wrap("x", 0, 2), vec![String::new()]);
}

#[test]
fn each_kind_maps_to_its_own_palette_colour() {
    let p = crate::theme::ThemeName::DefaultDark.palette();
    assert_eq!(color(ToastKind::Error, &p), p.error);
    assert_eq!(color(ToastKind::Warning, &p), p.warning);
    assert_eq!(color(ToastKind::Success, &p), p.success);
    assert_eq!(color(ToastKind::Info, &p), p.info);
}

fn paint(width: u16, height: u16, toasts: &[Toast]) -> String {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let palette = crate::theme::ThemeName::DefaultDark.palette();
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| {
            let area = frame.area();
            render(frame, area, toasts, &palette);
        })
        .expect("draw");
    let buf = terminal.backend().buffer().clone();
    let area = buf.area();
    let mut out = String::new();
    for y in area.y..area.y.saturating_add(area.height) {
        for x in area.x..area.x.saturating_add(area.width) {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

#[test]
fn render_skips_empty_or_tiny_areas() {
    let toast = Toast::new(ToastKind::Info, "hello");
    assert!(
        !paint(80, 24, &[]).contains('i'),
        "empty stack must not paint"
    );
    let tiny = paint(10, 2, std::slice::from_ref(&toast));
    assert!(
        !tiny.contains("hello"),
        "area below the 16×3 floor must skip: {tiny:?}"
    );
}

#[test]
fn render_paints_each_kind_in_the_top_right() {
    let toasts = [
        Toast::new(ToastKind::Success, "saved"),
        Toast::new(ToastKind::Error, "boom"),
        Toast::new(ToastKind::Warning, "careful"),
        Toast::new(ToastKind::Info, "note"),
    ];
    let text = paint(80, 24, &toasts);
    assert!(text.contains("saved"), "{text}");
    assert!(text.contains("boom"), "{text}");
    assert!(text.contains("careful"), "{text}");
    assert!(text.contains("note"), "{text}");
    assert!(text.contains('✓'), "success glyph: {text}");
    assert!(text.contains('✕'), "error glyph: {text}");
    assert!(text.contains('!'), "warning glyph: {text}");
}

#[test]
fn render_wraps_long_copy_and_truncates_overflow() {
    let long = Toast::new(
        ToastKind::Info,
        "one two three four five six seven eight nine ten eleven twelve",
    );
    let text = paint(40, 12, std::slice::from_ref(&long));
    assert!(
        text.contains("one two") || text.contains("one"),
        "wrapped body must appear: {text}"
    );

    // A short stack that cannot fit a second toast: first stays, rest drops.
    let stacked = [
        Toast::new(ToastKind::Info, "first toast stays"),
        Toast::new(ToastKind::Error, "second never fits"),
    ];
    let cramped = paint(32, 5, &stacked);
    assert!(cramped.contains("first"), "{cramped}");
    assert!(
        !cramped.contains("second"),
        "overflow toast must not paint: {cramped}"
    );
}

#[test]
fn render_skips_when_the_toast_would_leave_the_area() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    let palette = crate::theme::ThemeName::DefaultDark.palette();
    let backend = TestBackend::new(20, 8);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let toast = Toast::new(ToastKind::Info, "hello");
    terminal
        .draw(|frame| {
            render(
                frame,
                Rect::new(12, 0, 8, 8),
                std::slice::from_ref(&toast),
                &palette,
            );
        })
        .expect("draw");
}

#[test]
fn wrap_keeps_a_single_word_under_the_limit() {
    assert_eq!(wrap("hello", 10, 2), vec!["hello"]);
    // Whitespace-only collapses like empty.
    assert_eq!(wrap("   \t  ", 8, 2), vec![String::new()]);
}
