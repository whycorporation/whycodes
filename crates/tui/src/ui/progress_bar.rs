//! Unicode progress bar at 1/8-cell resolution (Grok `progress_bar`).
//!
//! Full cells use `█`; partial fill uses LEFT fractional blocks
//! `▏▎▍▌▋▊▉`; empty track uses `░` so the bar stays visible without a
//! background wash.

/// LEFT-fractional block glyphs, index 0–8 (0 empty, 8 full).
const BLOCKS: [&str; 9] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉", "█"];

/// Split fill into (whole cells, remainder eighths).
fn cell_breakdown(width: u16, value: f64) -> (u16, usize) {
    let value = value.clamp(0.0, 1.0);
    let total_eighths = (value * width as f64 * 8.0).round() as u16;
    let full = (total_eighths / 8).min(width);
    let remainder = (total_eighths % 8) as usize;
    (full, remainder)
}

/// Build a progress bar string of exactly `width` display cells.
///
/// `value` is fill fraction `0.0..=1.0`.
pub fn progress_bar_string(width: u16, value: f64) -> String {
    if width == 0 {
        return String::new();
    }
    let (full, remainder) = cell_breakdown(width, value);
    let mut s = String::with_capacity(width as usize * 3);
    for i in 0..width {
        if i < full {
            s.push_str(BLOCKS[8]);
        } else if i == full && remainder > 0 {
            s.push_str(BLOCKS[remainder]);
        } else {
            s.push('░');
        }
    }
    s
}

/// Linear RGB blend for urgency gradients (Grok context bar breakpoints).
///
/// Named/ANSI colors fall back to approximate RGB so themes without truecolor
/// still get a smooth-ish ramp.
pub fn blend_rgb(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    let r = (a.0 as f32 + (b.0 as f32 - a.0 as f32) * t).round() as u8;
    let g = (a.1 as f32 + (b.1 as f32 - a.1 as f32) * t).round() as u8;
    let bch = (a.2 as f32 + (b.2 as f32 - a.2 as f32) * t).round() as u8;
    (r, g, bch)
}

/// Map context fill percent (0–100+) to an RGB urgency color.
///
/// Breakpoints (Grok-inspired): dim → accent → warning → error.
pub fn context_urgency_rgb(
    pct: f64,
    dim: (u8, u8, u8),
    accent: (u8, u8, u8),
    warning: (u8, u8, u8),
    error: (u8, u8, u8),
) -> (u8, u8, u8) {
    let bps: [(f64, (u8, u8, u8)); 4] =
        [(0.0, dim), (50.0, accent), (75.0, warning), (95.0, error)];
    if pct <= bps[0].0 {
        return bps[0].1;
    }
    for i in 1..bps.len() {
        if pct <= bps[i].0 {
            let t = ((pct - bps[i - 1].0) / (bps[i].0 - bps[i - 1].0)) as f32;
            return blend_rgb(bps[i - 1].1, bps[i].1, t);
        }
    }
    bps[bps.len() - 1].1
}

/// Best-effort RGB for a ratatui color (truecolor or common named).
pub fn color_to_rgb(c: ratatui::style::Color) -> (u8, u8, u8) {
    use ratatui::style::Color;
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (0, 0, 0),
        Color::Red => (205, 49, 49),
        Color::Green => (13, 188, 121),
        Color::Yellow => (229, 229, 16),
        Color::Blue => (36, 114, 200),
        Color::Magenta => (188, 63, 188),
        Color::Cyan => (17, 168, 205),
        Color::Gray | Color::DarkGray => (128, 128, 128),
        Color::White => (229, 229, 229),
        Color::LightRed => (241, 76, 76),
        Color::LightGreen => (35, 209, 139),
        Color::LightYellow => (245, 245, 67),
        Color::LightBlue => (59, 142, 234),
        Color::LightMagenta => (214, 112, 214),
        Color::LightCyan => (41, 184, 219),
        Color::Indexed(i) => indexed_approx(i),
        Color::Reset => (180, 180, 180),
    }
}

fn indexed_approx(i: u8) -> (u8, u8, u8) {
    // 16-color + xterm 256 rough map for the common range.
    if i < 16 {
        return color_to_rgb(match i {
            0 => ratatui::style::Color::Black,
            1 => ratatui::style::Color::Red,
            2 => ratatui::style::Color::Green,
            3 => ratatui::style::Color::Yellow,
            4 => ratatui::style::Color::Blue,
            5 => ratatui::style::Color::Magenta,
            6 => ratatui::style::Color::Cyan,
            7 => ratatui::style::Color::Gray,
            8 => ratatui::style::Color::DarkGray,
            9 => ratatui::style::Color::LightRed,
            10 => ratatui::style::Color::LightGreen,
            11 => ratatui::style::Color::LightYellow,
            12 => ratatui::style::Color::LightBlue,
            13 => ratatui::style::Color::LightMagenta,
            14 => ratatui::style::Color::LightCyan,
            _ => ratatui::style::Color::White,
        });
    }
    if (16..232).contains(&i) {
        let n = i - 16;
        let r = n / 36;
        let g = (n % 36) / 6;
        let b = n % 6;
        let step = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
        return (step(r), step(g), step(b));
    }
    let gray = 8 + 10 * (i.saturating_sub(232));
    (gray, gray, gray)
}

#[cfg(test)]
mod tests {
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
}
