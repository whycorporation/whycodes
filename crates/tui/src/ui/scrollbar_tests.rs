use super::*;

#[test]
fn paint_scrollbar_marks_thumb_cells() {
    let area = Rect::new(0, 0, 10, 6);
    let mut buf = Buffer::empty(area);
    let sb = Rect::new(9, 0, 1, 6);
    paint_scrollbar(
        &mut buf,
        sb,
        15,
        6,
        0,
        Color::Rgb(20, 20, 20),
        Color::Rgb(90, 90, 90),
    );
    let thumb_bg = Color::Rgb(90, 90, 90);
    let track_bg = Color::Rgb(20, 20, 20);
    let top = buf.cell((9, 0)).expect("top cell");
    assert_eq!(top.bg, thumb_bg, "top cell should be thumb at offset 0");
    let bottom = buf.cell((9, 5)).expect("bottom cell");
    assert_eq!(bottom.bg, track_bg, "bottom cell should be track");
}

#[test]
fn scroll_to_selected_keeps_cursor_visible() {
    assert_eq!(scroll_to_selected(0, 20, 5), 0);
    assert_eq!(scroll_to_selected(4, 20, 5), 0);
    assert_eq!(scroll_to_selected(5, 20, 5), 1);
    assert_eq!(scroll_to_selected(19, 20, 5), 15);
    assert_eq!(scroll_to_selected(0, 3, 5), 0);
}

#[test]
fn scroll_center_centers_selection() {
    assert_eq!(scroll_center(0, 20, 6), 0);
    assert_eq!(scroll_center(2, 20, 6), 0);
    assert_eq!(scroll_center(10, 20, 6), 7);
    assert_eq!(scroll_center(19, 20, 6), 14);
    assert_eq!(scroll_center(0, 4, 6), 0);
}

#[test]
fn selection_for_offset_pins_viewport_top() {
    // visible=5, offset=3 → selected at last visible row so scroll_to_selected == 3
    let sel = selection_for_offset(3, 20, 5);
    assert_eq!(scroll_to_selected(sel, 20, 5), 3);
    let sel0 = selection_for_offset(0, 20, 5);
    assert_eq!(scroll_to_selected(sel0, 20, 5), 0);
}

#[test]
fn offset_from_pointer_roundtrips_thumb_ends() {
    let track = Rect::new(10, 0, 1, 10);
    // Top of track → offset 0
    assert_eq!(offset_from_pointer_y(0, track, 30, 10, Some(0)), 0);
    // Bottom of track with grab at thumb bottom-ish
    let max_off = 20usize;
    let bottom = offset_from_pointer_y(9, track, 30, 10, Some(0));
    assert!(bottom <= max_off);
    assert!(bottom >= max_off.saturating_sub(2), "near end: {bottom}");
}

#[test]
fn thumb_is_flush_with_track_bottom_when_offset_is_max() {
    // Chat "at bottom" uses top-origin view_start = max_off. Thumb must
    // sit on the last cells of the track — not ~70% like ratatui::Scrollbar.
    let area = Rect::new(0, 0, 1, 20);
    let mut buf = Buffer::empty(area);
    let total = 100usize;
    let visible = 20usize;
    let max_off = total - visible;
    let thumb = Color::Rgb(200, 200, 200);
    let track = Color::Rgb(40, 40, 40);
    paint_scrollbar(&mut buf, area, total, visible, max_off, track, thumb);

    let (thumb_len, _, _) = scrollbar_metrics(total, visible, 20).unwrap();
    // Last cell of track must be thumb
    assert_eq!(
        buf.cell((0, 19)).map(|c| c.bg),
        Some(thumb),
        "bottom cell should be thumb at max offset"
    );
    // Cell just above thumb block should be track (if thumb doesn't fill all)
    if thumb_len < 20 {
        let above = 19u16 - thumb_len as u16;
        assert_eq!(
            buf.cell((0, above)).map(|c| c.bg),
            Some(track),
            "cell above thumb should be track"
        );
    }
    // Top cell should be track
    assert_eq!(buf.cell((0, 0)).map(|c| c.bg), Some(track));
}

#[test]
fn thumb_is_flush_with_track_top_when_offset_is_zero() {
    let area = Rect::new(0, 0, 1, 20);
    let mut buf = Buffer::empty(area);
    paint_scrollbar(
        &mut buf,
        area,
        100,
        20,
        0,
        Color::Rgb(40, 40, 40),
        Color::Rgb(200, 200, 200),
    );
    assert_eq!(
        buf.cell((0, 0)).map(|c| c.bg),
        Some(Color::Rgb(200, 200, 200)),
        "top cell should be thumb at offset 0"
    );
}

#[test]
fn scrollbar_metrics_and_content_helpers() {
    assert!(scrollbar_metrics(5, 10, 10).is_none());
    assert_eq!(thumb_top_for_offset(0, 0, 0), 0);
    assert_eq!(offset_for_thumb_top(0, 0, 0), 0);
    assert_eq!(scroll_to_selected(0, 0, 0), 0);
    assert_eq!(selection_for_offset(0, 0, 5), 0);
    let track = Rect::new(0, 2, 1, 8);
    assert!(scrollbar_contains(track, 0, 2));
    assert!(!scrollbar_contains(track, 1, 2));
    let area = Rect::new(0, 0, 10, 6);
    let mut buf = Buffer::empty(area);
    let colors = ScrollbarColors {
        track: Color::Rgb(20, 20, 20),
        thumb: Color::Rgb(90, 90, 90),
    };
    let content = content_with_scrollbar(&mut buf, area, 3, 10, 0, colors);
    assert_eq!(content, area);
    let _ = elevate(Color::Black, 8);
    let _ = contrast_ok(Color::Black, Color::White);
    let _ = to_rgb(Color::Reset);
    for c in [
        Color::Black,
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::Gray,
        Color::DarkGray,
        Color::LightRed,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightBlue,
        Color::LightMagenta,
        Color::LightCyan,
        Color::White,
        Color::Indexed(5),
        Color::Indexed(16),
        Color::Indexed(232),
        Color::Indexed(196),
    ] {
        let _ = to_rgb(c);
    }
    let palette = crate::theme::ThemeName::DefaultDark.palette();
    let colors = ScrollbarColors::from_palette(&palette);
    assert_ne!(colors.track, colors.thumb);
    let mut low = palette.clone();
    low.scrollbar = palette.bg;
    let dim_thumb = ScrollbarColors::from_palette(&low);
    assert_eq!(dim_thumb.thumb, palette.dim);
    low.dim = palette.bg;
    let fallback = ScrollbarColors::from_palette(&low);
    assert_ne!(fallback.track, fallback.thumb);
    let mut buf_full = Buffer::empty(Rect::new(0, 0, 1, 20));
    paint_scrollbar(
        &mut buf_full,
        Rect::new(0, 0, 1, 20),
        21,
        20,
        0,
        colors.track,
        colors.thumb,
    );
    assert_eq!(
        buf_full.cell((0, 0)).map(|c| c.bg),
        Some(colors.thumb),
        "thumb fills the track when it is as tall as the viewport"
    );
    let mut buf = Buffer::empty(area);
    let shrunk = content_with_scrollbar(&mut buf, area, 20, 4, 0, colors);
    assert!(shrunk.width < area.width);
    paint_scrollbar(
        &mut buf,
        Rect::new(0, 0, 0, 4),
        20,
        4,
        0,
        colors.track,
        colors.thumb,
    );
    assert_eq!(
        offset_from_pointer_y(0, Rect::new(0, 2, 1, 8), 5, 10, None),
        0
    );
    let track = Rect::new(0, 2, 1, 8);
    assert_eq!(offset_from_pointer_y(1, track, 30, 10, Some(0)), 0);
    let _ = offset_from_pointer_y(20, track, 30, 10, None);
}
