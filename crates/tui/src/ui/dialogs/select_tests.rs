use super::*;

#[test]
fn an_item_can_carry_a_detail() {
    let plain = SelectItem::new("a");
    assert_eq!(plain.label, "a");
    assert!(plain.detail.is_none());

    let detailed = SelectItem::with_detail("a", "b");
    assert_eq!(detailed.detail.as_deref(), Some("b"));
}

#[test]
fn paint_helpers_skip_zero_size_rows() {
    use crate::theme::ThemeName;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    let palette = ThemeName::DefaultDark.palette();
    let mut buf = Buffer::empty(Rect::new(0, 0, 10, 2));
    paint_picker_row(
        &mut buf,
        Rect::new(0, 0, 0, 1),
        "x",
        None,
        false,
        &palette,
        false,
    );
    paint_section_header(&mut buf, Rect::new(0, 0, 0, 1), "H", &palette);
    let backend = ratatui::backend::TestBackend::new(10, 4);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            paint_search_bar(f, Rect::new(0, 0, 0, 1), "q", true, &palette);
            let _ = render_select(f, "Pick", &[], 0, "empty", &palette, None);
        })
        .unwrap();
}
