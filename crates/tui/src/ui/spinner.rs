//! Busy-indicator frames shared by the header, turn strip, and tasks.
//!
//! Braille dots (`⠋⠙…`) are one cell in a capable font, but Windows
//! consoles (and some OEM code pages) draw them as a bright tofu / white
//! flash on every frame. ASCII `| / - \` stays one cell everywhere.

use ratatui::style::{Color, Style};

pub const FRAMES: &[&str] = &["|", "/", "-", "\\"];

const LEGACY_BRAILLE: &str = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";

pub fn glyph(frame: usize) -> &'static str {
    FRAMES[frame % FRAMES.len()]
}

pub fn is_spinner_char(c: char) -> bool {
    matches!(c, '|' | '/' | '-' | '\\') || LEGACY_BRAILLE.contains(c)
}

/// Foreground on an explicit canvas so a span cannot leak `Color::Reset`
/// (host default, often white) over the theme background.
pub fn fg(fg: Color, bg: Color) -> Style {
    Style::default().fg(fg).bg(bg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    #[test]
    fn glyphs_are_ascii_and_cycle() {
        assert_eq!(glyph(0), "|");
        assert_eq!(glyph(1), "/");
        assert_eq!(glyph(2), "-");
        assert_eq!(glyph(3), "\\");
        assert_eq!(glyph(4), "|");
        assert!(is_spinner_char('|'));
        assert!(is_spinner_char('⠋'));
        assert!(!is_spinner_char('G'));
    }

    #[test]
    fn fg_pins_canvas_without_bold() {
        let s = fg(Color::Rgb(1, 2, 3), Color::Rgb(18, 18, 24));
        assert_eq!(s.fg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(s.bg, Some(Color::Rgb(18, 18, 24)));
        assert!(!s.add_modifier.contains(Modifier::BOLD));
    }
}
