use super::*;

#[test]
fn split_shortcut_separates_key_and_label() {
    assert_eq!(split_shortcut_label("Esc cancel"), ("Esc", " cancel"));
    assert_eq!(split_shortcut_label("Enter"), ("Enter", ""));
    assert_eq!(split_shortcut_label("↑/↓ move"), ("↑/↓", " move"));
}

#[test]
fn popup_rect_matches_grok_shortcuts_sizing() {
    // 80-col: 70% is 56, inside [44, 80].
    let r = popup_rect(Rect::new(0, 0, 80, 40), DialogSizing::popup());
    assert_eq!(r.width, 56);
    assert_eq!(r.height, 32); // 40 - 2*4
    assert_eq!(r.x, 12);
    assert_eq!(r.y, 4);

    // Wide terminal: 70% of 120 is 84, capped at 80.
    let wide = popup_rect(Rect::new(0, 0, 120, 40), DialogSizing::popup());
    assert_eq!(wide.width, 80);
    assert_eq!(wide.x, 20);

    // Phone: min_width 44 exceeds 40 → fill the PTY.
    let phone = popup_rect(Rect::new(0, 0, 40, 24), DialogSizing::popup());
    assert_eq!(phone.width, 40);
    assert_eq!(phone.height, 16);
}

#[test]
fn close_button_sits_on_top_right_of_modal() {
    let modal = Rect {
        x: 10,
        y: 5,
        width: 40,
        height: 20,
    };
    let hit = close_button_rect(modal).expect("wide enough");
    assert_eq!(hit.y, modal.y);
    assert_eq!(hit.height, 1);
    // Glyph starts 2 inset; hit extends to the right border edge.
    assert_eq!(hit.x, modal.x + modal.width - CLOSE_GLYPH_W - 2);
    assert_eq!(hit.x + hit.width, modal.x + modal.width);
    assert!(hit.width >= CLOSE_GLYPH_W);
}
