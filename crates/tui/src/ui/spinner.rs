//! Busy-indicator frames shared by the header, turn strip, and tasks.
//!
//! Grok Build uses 1-column braille (`⠋⠙⠹⠸⠼⠴⠦⠧`) at ~7.5 fps. Those
//! code points are missing from CP437, so legacy Windows ConHost falls
//! back to ASCII `| / - \`. Both sets stay one cell so a frame tick
//! cannot shift the label or timer that follows.

use ratatui::style::{Color, Style};
use std::sync::OnceLock;

/// Grok `braille_spinner_frames` fancy set (U+280B … U+2827).
pub const BRAILLE_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

/// Legacy ConHost 1-column fallback.
pub const ASCII_FRAMES: &[&str] = &["|", "/", "-", "\\"];

/// Animate-loop dwell: `REDRAW_ANIMATE` is 40 ms, Grok shows each glyph
/// ~133 ms (~7.5 fps). Three 40 ms ticks ≈ 120 ms.
pub const TICK_DIVISOR: u32 = 3;

pub fn frames() -> &'static [&'static str] {
    if is_legacy_windows_console() {
        ASCII_FRAMES
    } else {
        BRAILLE_FRAMES
    }
}

pub fn frame_count() -> usize {
    frames().len()
}

pub fn glyph(frame: usize) -> &'static str {
    let frames = frames();
    frames[frame % frames.len()]
}

pub fn is_spinner_char(c: char) -> bool {
    matches!(c, '|' | '/' | '-' | '\\')
        || BRAILLE_FRAMES.iter().any(|f| f.starts_with(c))
        // Older 10-frame writers also used `⠇⠏`; still strip them from status.
        || matches!(c, '⠇' | '⠏')
}

/// Cached: native Windows console whose font lacks braille chrome.
/// `WHYCODES_FORCE_LEGACY_CONSOLE` overrides so tests can pin either set.
pub fn is_legacy_windows_console() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| {
        forced_legacy_console_override().unwrap_or_else(detect_legacy_windows_console)
    })
}

fn forced_legacy_console_override() -> Option<bool> {
    parse_legacy_console_override(
        std::env::var("WHYCODES_FORCE_LEGACY_CONSOLE")
            .ok()
            .as_deref(),
    )
}

fn parse_legacy_console_override(raw: Option<&str>) -> Option<bool> {
    match raw {
        Some("1" | "true") => Some(true),
        Some("0" | "false") => Some(false),
        _ => None,
    }
}

fn detect_legacy_windows_console() -> bool {
    detect_legacy_windows_console_with(cfg!(windows), |k| std::env::var_os(k), |k| std::env::var(k))
}

/// Host detection split out so Linux CI can drive the Windows arms.
fn detect_legacy_windows_console_with(
    is_windows: bool,
    var_os: impl Fn(&str) -> Option<std::ffi::OsString>,
    var: impl Fn(&str) -> Result<String, std::env::VarError>,
) -> bool {
    if !is_windows {
        return false;
    }
    if var_os("WT_SESSION").is_some() {
        return false;
    }
    if var("ConEmuANSI")
        .ok()
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("ON"))
    {
        return false;
    }
    if let Ok(term_program) = var("TERM_PROGRAM") {
        let t = term_program.to_ascii_lowercase();
        if matches!(
            t.as_str(),
            "vscode"
                | "cursor"
                | "wezterm"
                | "ghostty"
                | "alacritty"
                | "kitty"
                | "zed"
                | "warp"
                | "windows_terminal"
        ) {
            return false;
        }
    }
    if let Ok(term) = var("TERM") {
        let t = term.to_ascii_lowercase();
        if t.contains("xterm")
            || t.contains("alacritty")
            || t.contains("kitty")
            || t.contains("wezterm")
            || t.contains("ghostty")
        {
            return false;
        }
    }
    true
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
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn braille_frames_match_grok_order_and_are_one_column() {
        assert_eq!(BRAILLE_FRAMES, &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"]);
        assert_eq!(BRAILLE_FRAMES[0], "\u{280b}");
        assert_eq!(BRAILLE_FRAMES[1], "\u{2819}");
        assert_eq!(BRAILLE_FRAMES[2], "\u{2839}");
        assert_eq!(BRAILLE_FRAMES[3], "\u{2838}");
        assert_eq!(BRAILLE_FRAMES[4], "\u{283c}");
        assert_eq!(BRAILLE_FRAMES[5], "\u{2834}");
        assert_eq!(BRAILLE_FRAMES[6], "\u{2826}");
        assert_eq!(BRAILLE_FRAMES[7], "\u{2827}");
        for frame in BRAILLE_FRAMES {
            assert_eq!(frame.width(), 1, "braille {frame:?} must be 1 column");
            let ch = frame.chars().next().expect("glyph");
            assert!(is_spinner_char(ch));
        }
    }

    #[test]
    fn ascii_fallback_is_one_column_and_cycles() {
        assert_eq!(ASCII_FRAMES, &["|", "/", "-", "\\"]);
        for frame in ASCII_FRAMES {
            assert_eq!(frame.width(), 1, "ascii {frame:?} must be 1 column");
        }
        assert!(is_spinner_char('|'));
        assert!(is_spinner_char('/'));
        assert!(!is_spinner_char('G'));
    }

    #[test]
    fn glyph_indexes_the_active_set() {
        let set = frames();
        assert_eq!(glyph(0), set[0]);
        assert_eq!(glyph(set.len()), set[0]);
        assert_eq!(frame_count(), set.len());
    }

    #[test]
    fn fg_pins_canvas_without_bold() {
        let s = fg(Color::Rgb(1, 2, 3), Color::Rgb(18, 18, 24));
        assert_eq!(s.fg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(s.bg, Some(Color::Rgb(18, 18, 24)));
        assert!(!s.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn forced_legacy_console_override_reads_env_values() {
        assert_eq!(parse_legacy_console_override(Some("1")), Some(true));
        assert_eq!(parse_legacy_console_override(Some("true")), Some(true));
        assert_eq!(parse_legacy_console_override(Some("0")), Some(false));
        assert_eq!(parse_legacy_console_override(Some("false")), Some(false));
        assert_eq!(parse_legacy_console_override(Some("maybe")), None);
        assert_eq!(parse_legacy_console_override(None), None);
        assert!(!detect_legacy_windows_console_with(
            false,
            |_| None,
            |_| { Err(std::env::VarError::NotPresent) }
        ));
        assert!(detect_legacy_windows_console_with(
            true,
            |_| None,
            |_| { Err(std::env::VarError::NotPresent) }
        ));
        assert!(!detect_legacy_windows_console_with(
            true,
            |k| {
                if k == "WT_SESSION" {
                    Some(std::ffi::OsString::from("1"))
                } else {
                    None
                }
            },
            |_| Err(std::env::VarError::NotPresent),
        ));
        assert!(!detect_legacy_windows_console_with(
            true,
            |_| None,
            |k| {
                if k == "ConEmuANSI" {
                    Ok("ON".into())
                } else {
                    Err(std::env::VarError::NotPresent)
                }
            },
        ));
        for program in [
            "vscode",
            "cursor",
            "wezterm",
            "ghostty",
            "alacritty",
            "kitty",
            "zed",
            "warp",
            "windows_terminal",
            "VSCODE",
        ] {
            assert!(
                !detect_legacy_windows_console_with(
                    true,
                    |_| None,
                    |k| {
                        if k == "TERM_PROGRAM" {
                            Ok(program.into())
                        } else {
                            Err(std::env::VarError::NotPresent)
                        }
                    },
                ),
                "{program}"
            );
        }
        for term in ["xterm-256color", "alacritty", "kitty", "wezterm", "ghostty"] {
            assert!(
                !detect_legacy_windows_console_with(
                    true,
                    |_| None,
                    |k| {
                        if k == "TERM" {
                            Ok(term.into())
                        } else {
                            Err(std::env::VarError::NotPresent)
                        }
                    },
                ),
                "{term}"
            );
        }
        assert!(detect_legacy_windows_console_with(
            true,
            |_| None,
            |k| {
                if k == "TERM_PROGRAM" {
                    Ok("conhost".into())
                } else if k == "TERM" {
                    Ok("dumb".into())
                } else if k == "ConEmuANSI" {
                    Ok("off".into())
                } else {
                    Err(std::env::VarError::NotPresent)
                }
            },
        ));
        let _ = forced_legacy_console_override();
        let _ = detect_legacy_windows_console();
    }
}
