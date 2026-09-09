use super::*;
use ratatui::Terminal;
use ratatui::backend::{Backend, TestBackend};
use std::io::Write;

#[test]
fn cube_and_gray_round_trip() {
    for i in 16..=255u8 {
        let (r, g, b) = indexed_rgb(i);
        assert_eq!(
            rgb_to_indexed256(r, g, b),
            i,
            "index {i} rgb({r},{g},{b}) must round-trip"
        );
    }
}

#[test]
fn known_rgb_maps_to_cube() {
    // xterm cube vertices: 0 / 95 / 135 / 175 / 215 / 255
    assert_eq!(rgb_to_indexed256(255, 0, 0), 196); // 16 + 36*5
    assert_eq!(rgb_to_indexed256(0, 255, 0), 46); // 16 + 6*5
    assert_eq!(rgb_to_indexed256(0, 0, 255), 21); // 16 + 5
    assert_eq!(rgb_to_indexed256(255, 255, 255), 231);
    assert_eq!(rgb_to_indexed256(0, 0, 0), 16);
}

#[test]
fn quantize_drops_rgb_in_256_and_16() {
    let rgb = Color::Rgb(0x4a, 0x9e, 0xff);
    assert_eq!(quantize_color(rgb, ColorMode::TrueColor), rgb);
    assert!(matches!(
        quantize_color(rgb, ColorMode::Ansi256),
        Color::Indexed(_)
    ));
    assert!(matches!(
        quantize_color(rgb, ColorMode::Ansi16),
        Color::Indexed(i) if i <= 15
    ));
    assert_eq!(quantize_color(Color::Red, ColorMode::Ansi16), Color::Red);
    assert_eq!(
        quantize_color(Color::Indexed(3), ColorMode::Ansi16),
        Color::Indexed(3)
    );
    assert_eq!(
        quantize_color(Color::Reset, ColorMode::Ansi256),
        Color::Reset
    );
    assert_eq!(
        quantize_color(Color::Indexed(196), ColorMode::Ansi256),
        Color::Indexed(196)
    );
}

#[test]
fn ansi16_collapses_high_indexes() {
    let c = quantize_color(Color::Indexed(196), ColorMode::Ansi16);
    assert!(matches!(c, Color::Indexed(i) if i <= 15));
}

#[test]
fn apple_terminal_is_256_even_with_colorterm() {
    assert_eq!(
        color_mode_from_env(
            None,
            Some("truecolor"),
            Some("Apple_Terminal"),
            Some("xterm-256color"),
            false
        ),
        ColorMode::Ansi256
    );
    assert_eq!(
        color_mode_from_env(
            None,
            Some("truecolor"),
            Some("iTerm.app"),
            Some("xterm-256color"),
            false
        ),
        ColorMode::TrueColor
    );
    assert_eq!(
        color_mode_from_env(None, None, None, Some("xterm-256color"), false),
        ColorMode::Ansi256
    );
    assert_eq!(
        color_mode_from_env(None, None, None, Some("xterm"), false),
        ColorMode::Ansi16
    );
    assert_eq!(
        color_mode_from_env(
            Some("16"),
            Some("truecolor"),
            None,
            Some("alacritty"),
            false
        ),
        ColorMode::Ansi16
    );
    assert_eq!(
        color_mode_from_env(
            Some("truecolor"),
            None,
            Some("Apple_Terminal"),
            Some("xterm-256color"),
            false
        ),
        ColorMode::TrueColor
    );
    assert_eq!(
        color_mode_from_env(Some("24bit"), None, None, Some("xterm"), false),
        ColorMode::TrueColor
    );
    assert_eq!(
        color_mode_from_env(Some("24"), None, None, Some("xterm"), false),
        ColorMode::TrueColor
    );
    assert_eq!(
        color_mode_from_env(Some("ansi256"), None, None, Some("xterm"), false),
        ColorMode::Ansi256
    );
    assert_eq!(
        color_mode_from_env(Some("ansi"), None, None, Some("xterm-256color"), false),
        ColorMode::Ansi16
    );
    assert_eq!(
        color_mode_from_env(Some("nope"), None, None, Some("xterm-direct"), false),
        ColorMode::TrueColor
    );
    assert_eq!(
        color_mode_from_env(Some("  "), None, None, Some("rxvt-unicode"), false),
        ColorMode::Ansi16
    );
    assert_eq!(
        color_mode_from_env(None, Some("24bit"), None, Some("xterm"), false),
        ColorMode::TrueColor
    );
    assert_eq!(
        color_mode_from_env(None, None, None, Some("dumb"), false),
        ColorMode::Ansi16
    );
    assert_eq!(
        color_mode_from_env(None, None, None, Some("unknown"), false),
        ColorMode::Ansi16
    );
    assert_eq!(
        color_mode_from_env(None, None, None, Some("screen-256colour"), false),
        ColorMode::Ansi256
    );
    assert_eq!(
        color_mode_from_env(None, Some("yes"), None, Some("vt100-color"), false),
        ColorMode::Ansi16
    );
    assert_eq!(
        color_mode_from_env(None, None, None, Some("ansi"), false),
        ColorMode::Ansi16
    );
}

#[test]
fn windows_empty_term_is_truecolor() {
    // Windows Terminal / PowerShell: no TERM, no COLORTERM.
    assert_eq!(
        color_mode_from_env(None, None, None, None, true),
        ColorMode::TrueColor
    );
    assert_eq!(
        color_mode_from_env(None, None, None, Some(""), true),
        ColorMode::TrueColor
    );
    // Unix with empty TERM stays 16-colour (real dumb / unknown host).
    assert_eq!(
        color_mode_from_env(None, None, None, None, false),
        ColorMode::Ansi16
    );
    // WHYCODES_COLOR still wins on Windows.
    assert_eq!(
        color_mode_from_env(Some("16"), None, None, None, true),
        ColorMode::Ansi16
    );
    assert!(windows_truecolor_from_env(true, None, false));
    assert!(windows_truecolor_from_env(false, Some("ON"), false));
    assert!(windows_truecolor_from_env(false, None, true));
    assert!(!windows_truecolor_from_env(false, Some("OFF"), false));
}

#[test]
fn ansi16_keeps_chroma_off_gray() {
    // Default-dark success / peach / info — the agent border + accent
    // tokens. Euclidean-nearest of the 16 used to be silver (7).
    for (r, g, b) in [
        (0x7f, 0xd8, 0x8f), // success / build
        (0xfa, 0xb2, 0x83), // peach primary
        (0x5c, 0x9c, 0xf5), // secondary / ask
        (0x9d, 0x7c, 0xd8), // accent / plan
    ] {
        let i = rgb_to_ansi16(r, g, b);
        assert!(
            !matches!(i, 0 | 7 | 8 | 15),
            "chromatic rgb({r},{g},{b}) mapped to gray index {i}"
        );
    }
    // Real greys may still land on the gray slots.
    assert!(matches!(rgb_to_ansi16(0x80, 0x80, 0x80), 0 | 7 | 8 | 15));
    assert!(matches!(rgb_to_ansi16(0x48, 0x48, 0x48), 0 | 7 | 8 | 15));
}

#[test]
fn quantizing_backend_rewrites_rgb_on_draw() {
    let inner = TestBackend::new(4, 1);
    let mut term =
        Terminal::new(QuantizingBackend::new(inner, ColorMode::Ansi256)).expect("test terminal");
    term.draw(|f| {
        let area = f.area();
        let buf = f.buffer_mut();
        if let Some(cell) = buf.cell_mut((area.x, area.y)) {
            cell.set_fg(Color::Rgb(255, 0, 0));
            cell.set_bg(Color::Rgb(0, 0, 0));
            cell.set_char('x');
        }
    })
    .expect("draw");
    let buf = term.backend().inner.buffer();
    let cell = buf.cell((0, 0)).expect("cell");
    assert!(
        !matches!(cell.fg, Color::Rgb(_, _, _)),
        "fg must not stay Rgb, got {:?}",
        cell.fg
    );
    assert!(
        !matches!(cell.bg, Color::Rgb(_, _, _)),
        "bg must not stay Rgb, got {:?}",
        cell.bg
    );
    assert_eq!(cell.fg, Color::Indexed(196));
}

#[test]
fn paint_color_follows_thread_local_mode() {
    let _g = push_color_mode(ColorMode::Ansi256);
    assert!(matches!(paint_rgb(255, 0, 0), Color::Indexed(196)));
}

#[test]
fn color_mode_and_named_rgb_helpers() {
    assert_eq!(ColorMode::TrueColor.as_str(), "truecolor");
    assert_eq!(ColorMode::Ansi256.as_str(), "256");
    assert_eq!(ColorMode::Ansi16.as_str(), "16");
    assert!(ColorMode::TrueColor.is_truecolor());
    assert!(!ColorMode::Ansi16.is_truecolor());
    assert_eq!(named_rgb(0), (0, 0, 0));
    assert_eq!(named_rgb(15), (255, 255, 255));
    assert_eq!(named_rgb(99), (255, 255, 255));
    let _ = detect_color_mode();
    let mut writer = QuantizingBackend::new(Vec::<u8>::new(), ColorMode::TrueColor);
    assert_eq!(writer.write(b"ok").unwrap(), 2);
    assert!(writer.flush().is_ok());

    let inner = TestBackend::new(4, 1);
    let mut term =
        Terminal::new(QuantizingBackend::new(inner, ColorMode::TrueColor)).expect("term");
    term.draw(|f| {
        let area = f.area();
        if let Some(cell) = f.buffer_mut().cell_mut((area.x, area.y)) {
            cell.set_fg(Color::Rgb(1, 2, 3));
            cell.set_char('x');
        }
    })
    .expect("draw");
    assert!(matches!(
        term.backend().inner.buffer().cell((0, 0)).unwrap().fg,
        Color::Rgb(1, 2, 3)
    ));
    let _ = term.backend_mut().size();
    let _ = term.backend_mut().window_size();
    let _ = term.backend_mut().hide_cursor();
    let _ = term.backend_mut().show_cursor();
    let _ = term.backend_mut().get_cursor_position();
    let _ = term.backend_mut().set_cursor_position((0, 0));
    let _ = term.backend_mut().clear();
    let _ = term
        .backend_mut()
        .clear_region(ratatui::backend::ClearType::All);
    let _ = term.backend_mut().append_lines(0);
    let _ = Backend::flush(term.backend_mut());
}
