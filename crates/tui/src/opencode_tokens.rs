//! Design tokens reverse-engineered from OpenCode TUI
//! (`anomalyco/opencode` packages/tui).
//!
//! Key layout rules (session/index.tsx + home.tsx + footer.tsx + border.ts):
//!
//! Session:
//!   row [ main | sidebar? ]
//!   main: paddingLeft=2, paddingRight=2, paddingBottom=1
//!     scrollbox (messages, sticky bottom)
//!     prompt (flexShrink=0)
//!
//! User message:
//!   border left only (┃), borderColor = agent color
//!   bg = backgroundPanel (#141414)
//!   paddingTop=1, paddingBottom=1, paddingLeft=2
//!   marginTop=1 between messages
//!
//! Assistant:
//!   free-flow parts, paddingLeft≈3 for tools
//!   epilogue: "▣ {agent} · {model}"
//!
//! Home:
//!   vertical center logo (4 rows) + gap + prompt (maxWidth 75 / 70%)
//!   footer optional
//!
//! Borders:
//!   EmptyBorder: no box on messages scroll
//!   SplitBorder: vertical "┃" only

use ratatui::style::Color;

pub mod dark {
    use super::Color;

    // theme/assets/opencode.json
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

/// OpenCode logo mark (logo.ts) — "OPEN" + "CODE" block font.
/// For whycode home we render this as brand homage with "whycode" subtitle,
/// or use LOGO_WHY / LOGO_CODE split.
pub const LOGO_OPEN: &[&str] = &[
    "                   ",
    "█▀▀█ █▀▀█ █▀▀█ █▀▀▄",
    "█  █ █  █ █▀▀▀ █  █",
    "▀▀▀▀ █▀▀▀ ▀▀▀▀ ▀▀▀▀",
];

pub const LOGO_CODE: &[&str] = &[
    "             ▄     ",
    "█▀▀▀ █▀▀█ █▀▀█ █▀▀█",
    "█    █  █ █  █ █▀▀ ",
    "▀▀▀▀ ▀▀▀▀ ▀▀▀▀ ▀▀▀▀",
];

/// whycode brand as two blocks: WHY + CODE (same visual language)
pub const LOGO_WHY: &[&str] = &[
    "                   ",
    "█   █ █   █ █   █  ",
    "█ █ █ █▀▀▀█ █▄▄▄█  ",
    "▀█▀█▀ █   █   █    ",
];

pub const LOGO_WHY_CODE: &[&str] = &[
    "             ▄     ",
    "█▀▀▀ █▀▀█ █▀▀█ █▀▀█",
    "█    █  █ █  █ █▀▀ ",
    "▀▀▀▀ ▀▀▀▀ ▀▀▀▀ ▀▀▀▀",
];

pub mod layout {
    /// home.tsx: maxWidth={75} or 70% of terminal
    pub const PROMPT_MAX_WIDTH: u16 = 75;
    pub const PROMPT_WIDTH_RATIO: f32 = 0.70;
    /// session main paddingLeft/Right = 2
    pub const SIDE_PAD: u16 = 2;
    /// user message paddingLeft = 2 (inside panel)
    pub const USER_PAD: u16 = 2;
    /// assistant tool paddingLeft = 3
    pub const ASSISTANT_PAD: u16 = 3;
    /// sidebar width ≈ 42 cols
    pub const SIDEBAR_WIDTH: u16 = 42;
}
