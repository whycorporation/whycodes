use super::*;
use ratatui::layout::Rect;

#[test]
fn inset_safe_shrinks_on_all_edges() {
    let area = Rect::new(4, 6, 100, 30);
    let inset = layout::inset_safe(area);
    assert_eq!(inset.x, area.x + layout::SAFE_LEFT);
    assert_eq!(inset.y, area.y + layout::SAFE_TOP);
    assert_eq!(
        inset.width,
        area.width - layout::SAFE_LEFT - layout::SAFE_RIGHT
    );
    assert_eq!(
        inset.height,
        area.height - layout::SAFE_TOP - layout::SAFE_BOTTOM
    );
}

#[test]
fn inset_safe_saturates_on_tiny_areas() {
    let area = Rect::new(0, 0, 1, 1);
    let inset = layout::inset_safe(area);
    assert_eq!(inset.width, 0);
    assert_eq!(inset.height, 0);
    assert_eq!(inset.x, 1);
    assert_eq!(inset.y, 1);
}

#[test]
fn below_header_leaves_a_row_under_the_status_bar() {
    let body = Rect::new(1, 2, 80, 20);
    let inset = layout::below_header(body);
    assert_eq!(inset.x, body.x);
    assert_eq!(inset.y, body.y + layout::TOP_PAD);
    assert_eq!(inset.width, body.width);
    assert_eq!(inset.height, body.height - layout::TOP_PAD);
}

#[test]
fn below_header_saturates_on_tiny_body() {
    let inset = layout::below_header(Rect::new(0, 0, 10, 0));
    assert_eq!(inset.height, 0);
    assert_eq!(inset.y, layout::TOP_PAD);
}

#[test]
fn layout_constants_leave_room_for_content() {
    // Prompt cap is strictly positive and the sidebar leaves space for
    // the main column at the minimum supported width.
    const {
        assert!(layout::PROMPT_MAX_WIDTH > 0);
        assert!(layout::PROMPT_WIDTH_RATIO > 0.0 && layout::PROMPT_WIDTH_RATIO < 1.0);
        assert!(layout::SIDEBAR_WIDTH > layout::SIDE_PAD * 2);
        assert!(layout::SIDE_PAD >= 2);
        assert!(layout::SIDE_PAD_NARROW >= 1);
        assert!(layout::PROMPT_MIN_WIDTH >= 8);
        assert!(layout::SIDEBAR_MIN_BODY > layout::SIDEBAR_MIN_CHAT);
        assert!(layout::HEADER_H >= 1);
        assert!(layout::CHAT_GAP >= 1);
        assert!(layout::PANEL_GAP >= 1);
    }
}

#[test]
fn popup_dim_matches_grok_max_percent_then_clamp() {
    // Grok: max(pct, min).min(outer). 90% of 40 is 36 — already at the floor.
    assert_eq!(layout::popup_dim(40, 90, 36), 36);
    // 50% of 40 is 20, floored to 36 then clamped to 40.
    assert_eq!(layout::popup_dim(40, 50, 36), 36);
    // Quarter pane (~20 cols): fill the PTY.
    assert_eq!(layout::popup_dim(20, 90, 36), 20);
    // Wide terminal: 90% of 120 is 108, above the 36 floor.
    assert_eq!(layout::popup_dim(120, 90, 36), 108);
    assert_eq!(layout::popup_dim(0, 90, 36), 0);
}

#[test]
fn home_logo_rows_are_uniform_width() {
    let mark_w: Vec<usize> = HOME_LOGO_MARK.iter().map(|l| l.chars().count()).collect();
    assert!(
        mark_w.windows(2).all(|w| w[0] == w[1]),
        "mark rows must stay aligned: {mark_w:?}"
    );
    assert!(mark_w[0] > 0);
}

#[test]
fn home_logo_is_question_mark() {
    // Bowl is open on the left (the cube-`?` cut); stem sits on the right.
    assert!(
        HOME_LOGO_MARK[2].trim_start().starts_with('█'),
        "stem must sit on the right of the bowl: {:?}",
        HOME_LOGO_MARK[2]
    );
    assert!(
        HOME_LOGO_MARK[5].chars().all(|c| c == ' '),
        "dot must be separated from the stem: {:?}",
        HOME_LOGO_MARK[5]
    );
    assert!(
        HOME_LOGO_MARK[6].contains('█'),
        "square dot must be present: {:?}",
        HOME_LOGO_MARK[6]
    );
    assert_eq!(HEADER_MARK, "?");
    assert_eq!(layout::HEADER_H, 1);
}

#[test]
fn dark_palette_constants_are_accessible() {
    // Compile-time sanity: the constants resolve to colors.
    let _ = dark::PRIMARY;
    let _ = dark::TEXT;
    let _ = dark::STEP1_BG;
    let _ = dark::ACCENT;
}
