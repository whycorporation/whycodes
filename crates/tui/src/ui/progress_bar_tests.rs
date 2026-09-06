use super::*;
use unicode_width::UnicodeWidthStr;

#[test]
fn bar_width_matches_request() {
    for w in [0u16, 1, 5, 11] {
        let s = progress_bar_string(w, 0.42);
        assert_eq!(s.width() as u16, w, "bar={s:?}");
    }
}

#[test]
fn bar_empty_and_full() {
    assert_eq!(progress_bar_string(4, 0.0), "░░░░");
    assert_eq!(progress_bar_string(4, 1.0), "████");
}

#[test]
fn urgency_ramps() {
    let dim = (100, 100, 100);
    let acc = (80, 160, 255);
    let warn = (220, 180, 40);
    let err = (220, 60, 60);
    let low = context_urgency_rgb(5.0, dim, acc, warn, err);
    let mid = context_urgency_rgb(50.0, dim, acc, warn, err);
    let hi = context_urgency_rgb(96.0, dim, acc, warn, err);
    assert_eq!(mid, acc);
    assert_eq!(hi, err);
    // low is near dim (blended slightly toward accent)
    assert!(low.0 <= acc.0 + 20);
}
