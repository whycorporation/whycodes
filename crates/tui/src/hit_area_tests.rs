use super::*;

fn rect(x: u16, y: u16, w: u16, h: u16) -> Rect {
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

#[test]
fn contains_respects_rect_and_empty() {
    let mut h = HitArea::default();
    assert!(!h.contains(0, 0));
    h.set_rect(Some(rect(2, 3, 4, 2)));
    assert!(h.contains(2, 3));
    assert!(h.contains(5, 4));
    assert!(!h.contains(6, 3));
    assert!(!h.contains(2, 5));
    assert!(!h.contains(1, 3));
}

#[test]
fn update_hover_flips_only_on_change() {
    let mut h = HitArea::default();
    h.set_rect(Some(rect(0, 0, 2, 2)));
    assert!(h.update_hover(0, 0));
    assert!(h.hovered);
    assert!(!h.update_hover(1, 1));
    assert!(h.update_hover(9, 9));
    assert!(!h.hovered);
}

#[test]
fn clear_drops_rect_and_hover() {
    let mut h = HitArea {
        rect: Some(rect(0, 0, 1, 1)),
        hovered: true,
    };
    h.clear();
    assert!(h.rect.is_none());
    assert!(!h.hovered);
    assert!(!h.contains(0, 0));
}
