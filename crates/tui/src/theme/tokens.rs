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
    /// Session main column horizontal padding.
    pub const SIDE_PAD: u16 = 2;
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
}
