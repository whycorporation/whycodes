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

fn row_text(buf: &ratatui::buffer::Buffer, y: u16) -> String {
    (0..buf.area.width)
        .map(|x| buf[(x, y)].symbol().to_string())
        .collect()
}

#[test]
fn group_header_and_children_read_as_a_hierarchy() {
    use crate::theme::ThemeName;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    let palette = ThemeName::DefaultDark.palette();
    let mut buf = Buffer::empty(Rect::new(0, 0, 30, 3));
    paint_group_header(
        &mut buf,
        Rect::new(0, 0, 30, 1),
        "anthropic",
        12,
        false,
        false,
        &palette,
    );
    paint_child_row(
        &mut buf,
        Rect::new(0, 1, 30, 1),
        "claude",
        None,
        true,
        &palette,
    );
    paint_group_header(
        &mut buf,
        Rect::new(0, 2, 30, 1),
        "openai",
        3,
        true,
        false,
        &palette,
    );

    let header = row_text(&buf, 0);
    assert!(header.starts_with("▾ anthropic"), "{header:?}");
    assert!(
        !header.contains('◆'),
        "headers carry no leaf mark: {header:?}"
    );
    assert!(
        header.trim_end().ends_with("12"),
        "count right-aligned: {header:?}"
    );
    assert_eq!(buf[(2, 0)].fg, palette.accent, "group name is accented");

    let child = row_text(&buf, 1);
    assert!(child.starts_with("  ◆ claude"), "child indented: {child:?}");

    assert!(row_text(&buf, 2).starts_with("▸ openai"));
}

#[test]
fn section_header_is_a_label_then_a_rule() {
    use crate::theme::ThemeName;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    let palette = ThemeName::DefaultDark.palette();
    let mut buf = Buffer::empty(Rect::new(0, 0, 12, 1));
    paint_section_header(&mut buf, Rect::new(0, 0, 12, 1), "Chat", &palette);
    assert_eq!(row_text(&buf, 0), "Chat ───────");
    assert_eq!(buf[(0, 0)].fg, palette.accent);
}

#[test]
fn narrow_rows_do_not_overflow() {
    use crate::theme::ThemeName;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    let palette = ThemeName::DefaultDark.palette();
    let mut buf = Buffer::empty(Rect::new(0, 0, 4, 2));
    paint_group_header(
        &mut buf,
        Rect::new(0, 0, 0, 1),
        "x",
        1,
        false,
        false,
        &palette,
    );
    paint_group_header(
        &mut buf,
        Rect::new(0, 0, 2, 1),
        "provider",
        100,
        false,
        false,
        &palette,
    );
    paint_child_row(
        &mut buf,
        Rect::new(0, 1, 2, 1),
        "model",
        None,
        false,
        &palette,
    );
    assert_eq!(row_text(&buf, 1), "    ");
}
