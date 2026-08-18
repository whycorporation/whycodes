//! TUI design tokens: default dark palette, layout metrics, home logo.

use ratatui::style::Color;

/// Default dark palette (step greys + semantic accents).
pub mod dark {
    use super::Color;

    pub const STEP1_BG: Color = Color::Rgb(0x0a, 0x0a, 0x0a);
    pub const STEP2_PANEL: Color = Color::Rgb(0x14, 0x14, 0x14);
    pub const STEP3_ELEMENT: Color = Color::Rgb(0x1e, 0x1e, 0x1e);
    pub const STEP6: Color = Color::Rgb(0x3c, 0x3c, 0x3c);
    pub const STEP7_BORDER: Color = Color::Rgb(0x48, 0x48, 0x48);
    pub const STEP8: Color = Color::Rgb(0x60, 0x60, 0x60);
    pub const PRIMARY: Color = Color::Rgb(0xfa, 0xb2, 0x83); // peach
    pub const PRIMARY_BRIGHT: Color = Color::Rgb(0xff, 0xc0, 0x9f);
    pub const SECONDARY: Color = Color::Rgb(0x5c, 0x9c, 0xf5); // blue
    pub const ACCENT: Color = Color::Rgb(0x9d, 0x7c, 0xd8); // purple
    pub const RED: Color = Color::Rgb(0xe0, 0x6c, 0x75);
    pub const ORANGE: Color = Color::Rgb(0xf5, 0xa7, 0x42);
    pub const GREEN: Color = Color::Rgb(0x7f, 0xd8, 0x8f);
    pub const CYAN: Color = Color::Rgb(0x56, 0xb6, 0xc2);
    pub const YELLOW: Color = Color::Rgb(0xe5, 0xc0, 0x7b);
    pub const TEXT: Color = Color::Rgb(0xee, 0xee, 0xee);
    pub const TEXT_MUTED: Color = Color::Rgb(0x80, 0x80, 0x80);
}

/// Home screen block logo: "WHY" + "CODE".
pub const HOME_LOGO_WHY: &[&str] = &[
    "                   ",
    "█   █ █   █ █   █  ",
    "█ █ █ █▀▀▀█ █▄▄▄█  ",
    "▀█▀█▀ █   █   █    ",
];

pub const HOME_LOGO_CODE: &[&str] = &[
    "             ▄     ",
    "█▀▀▀ █▀▀█ █▀▀█ █▀▀█",
    "█    █  █ █  █ █▀▀ ",
    "▀▀▀▀ ▀▀▀▀ ▀▀▀▀ ▀▀▀▀",
];

/// Spacing and chrome metrics shared by home / session shells.
pub mod layout {
    use ratatui::layout::Rect;

    /// Prompt max width: absolute cap, or fraction of terminal width.
    pub const PROMPT_MAX_WIDTH: u16 = 75;
    pub const PROMPT_WIDTH_RATIO: f32 = 0.70;
    /// Floor before the ratio/cap so portrait phones keep a usable prompt.
    pub const PROMPT_MIN_WIDTH: u16 = 24;
    /// Session main column horizontal padding (bubble left/right margin).
    pub const SIDE_PAD: u16 = 4;
    /// Tighter side pad on phone-portrait / keyboard-shrunk widths.
    pub const SIDE_PAD_NARROW: u16 = 1;
    /// Use [`SIDE_PAD_NARROW`] at or below this terminal body width.
    pub const NARROW_WIDTH: u16 = 56;

    /// Horizontal pad for the current body width.
    pub fn side_pad(width: u16) -> u16 {
        if width <= NARROW_WIDTH {
            SIDE_PAD_NARROW
        } else {
            SIDE_PAD
        }
    }
    /// Blank rows under the status header so chat does not sit flush on it.
    pub const TOP_PAD: u16 = 2;
    /// Blank rows between the transcript and the turn-status / prompt.
    pub const CHAT_GAP: u16 = 1;
    /// Gap under the prompt (bottom breathing room inside body).
    pub const BOTTOM_PAD: u16 = 1;
    /// Terminal edge insets (all four sides).
    pub const SAFE_TOP: u16 = 1;
    pub const SAFE_BOTTOM: u16 = 1;
    pub const SAFE_LEFT: u16 = 1;
    pub const SAFE_RIGHT: u16 = 1;
    /// Extra gap after a user message block.
    pub const USER_PAD: u16 = 1;
    /// Shared left gutter for tools / epilogue / meta under an assistant turn.
    pub const ASSISTANT_PAD: u16 = 2;
    /// Sidebar preferred width (clamped by terminal size at render time).
    pub const SIDEBAR_WIDTH: u16 = 42;
    /// Hide the sidebar below this body width so a tmux/iTerm split (½ or ¼
    /// of an 80-col PTY) keeps the transcript instead of a 24-col rail.
    /// Grok drops extra chrome the same way when the overlay cannot fit both.
    pub const SIDEBAR_MIN_BODY: u16 = 72;
    /// Chat column that must remain after the sidebar is reserved.
    pub const SIDEBAR_MIN_CHAT: u16 = 32;

    /// Grok popup formula: `max(percent of outer, min).min(outer)`.
    pub fn popup_dim(outer: u16, percent: u16, min: u16) -> u16 {
        if outer == 0 {
            return 0;
        }
        let pct = ((outer as u32 * percent as u32) / 100) as u16;
        pct.max(min.min(outer)).min(outer)
    }

    /// Shrink `area` by the safe-area insets on every edge.
    pub fn inset_safe(area: Rect) -> Rect {
        let h_pad = SAFE_LEFT.saturating_add(SAFE_RIGHT);
        let v_pad = SAFE_TOP.saturating_add(SAFE_BOTTOM);
        Rect {
            x: area.x.saturating_add(SAFE_LEFT),
            y: area.y.saturating_add(SAFE_TOP),
            width: area.width.saturating_sub(h_pad),
            height: area.height.saturating_sub(v_pad),
        }
    }

    /// Drop `TOP_PAD` from the top of the body so home/session/sidebar
    /// never paint into the status header sitting on the row above.
    pub fn below_header(body: Rect) -> Rect {
        Rect {
            x: body.x,
            y: body.y.saturating_add(TOP_PAD),
            width: body.width,
            height: body.height.saturating_sub(TOP_PAD),
        }
    }
}

#[cfg(test)]
mod tests {
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
            assert!(layout::SIDE_PAD >= 4);
            assert!(layout::SIDE_PAD_NARROW >= 1);
            assert!(layout::PROMPT_MIN_WIDTH >= 8);
            assert!(layout::SIDEBAR_MIN_BODY > layout::SIDEBAR_MIN_CHAT);
            assert!(layout::CHAT_GAP >= 1);
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
        let why_w: Vec<usize> = HOME_LOGO_WHY.iter().map(|l| l.chars().count()).collect();
        let code_w: Vec<usize> = HOME_LOGO_CODE.iter().map(|l| l.chars().count()).collect();
        assert!(why_w.windows(2).all(|w| w[0] == w[1]), "{why_w:?}");
        assert!(code_w.windows(2).all(|w| w[0] == w[1]), "{code_w:?}");
        assert!(why_w[0] > 0 && code_w[0] > 0);
    }

    #[test]
    fn dark_palette_constants_are_accessible() {
        // Compile-time sanity: the constants resolve to colors.
        let _ = dark::PRIMARY;
        let _ = dark::TEXT;
        let _ = dark::STEP1_BG;
        let _ = dark::ACCENT;
    }
}
