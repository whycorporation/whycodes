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
    assert_eq!(context_urgency_rgb(-1.0, dim, acc, warn, err), dim);
    let between = context_urgency_rgb(62.5, dim, acc, warn, err);
    assert_ne!(between, acc);
    assert_ne!(between, warn);
    assert_eq!(blend_rgb((0, 0, 0), (100, 100, 100), 2.0), (100, 100, 100));
    assert_eq!(blend_rgb((0, 0, 0), (100, 100, 100), -1.0), (0, 0, 0));
}

#[test]
fn bar_partial_cell_uses_fractional_block() {
    // 1/16 of 8 cells = 4/8 of the first cell → ▌
    let s = progress_bar_string(8, 0.0625);
    assert!(s.starts_with('▌'), "{s:?}");
    assert!(s.contains('░'), "{s:?}");
}

#[test]
fn color_to_rgb_covers_named_and_indexed() {
    use ratatui::style::Color;
    assert_eq!(color_to_rgb(Color::Rgb(1, 2, 3)), (1, 2, 3));
    assert_eq!(color_to_rgb(Color::Black), (0, 0, 0));
    assert_eq!(color_to_rgb(Color::Red), (205, 49, 49));
    assert_eq!(color_to_rgb(Color::Green), (13, 188, 121));
    assert_eq!(color_to_rgb(Color::Yellow), (229, 229, 16));
    assert_eq!(color_to_rgb(Color::Blue), (36, 114, 200));
    assert_eq!(color_to_rgb(Color::Magenta), (188, 63, 188));
    assert_eq!(color_to_rgb(Color::Cyan), (17, 168, 205));
    assert_eq!(color_to_rgb(Color::Gray), (128, 128, 128));
    assert_eq!(color_to_rgb(Color::DarkGray), (128, 128, 128));
    assert_eq!(color_to_rgb(Color::White), (229, 229, 229));
    assert_eq!(color_to_rgb(Color::LightRed), (241, 76, 76));
    assert_eq!(color_to_rgb(Color::LightGreen), (35, 209, 139));
    assert_eq!(color_to_rgb(Color::LightYellow), (245, 245, 67));
    assert_eq!(color_to_rgb(Color::LightBlue), (59, 142, 234));
    assert_eq!(color_to_rgb(Color::LightMagenta), (214, 112, 214));
    assert_eq!(color_to_rgb(Color::LightCyan), (41, 184, 219));
    assert_eq!(color_to_rgb(Color::Reset), (180, 180, 180));
    assert_eq!(color_to_rgb(Color::Indexed(0)), (0, 0, 0));
    assert_eq!(color_to_rgb(Color::Indexed(15)), (229, 229, 229));
    let cube = color_to_rgb(Color::Indexed(196));
    assert_eq!(cube, (255, 0, 0));
    let gray = color_to_rgb(Color::Indexed(232));
    assert_eq!(gray, (8, 8, 8));
}
