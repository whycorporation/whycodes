use super::*;
use crate::color::ColorMode;
use ratatui::style::Color;

#[test]
fn quantize_for_drops_rgb_on_256() {
    let mut p = ThemeName::DefaultDark.palette();
    assert!(matches!(p.accent, Color::Rgb(_, _, _)));
    p.quantize_for(ColorMode::Ansi256);
    for c in [
        p.bg,
        p.fg,
        p.accent,
        p.thinking,
        p.success,
        p.dim,
        p.dialog_bg,
    ] {
        assert!(
            !matches!(c, Color::Rgb(_, _, _)),
            "role still Rgb after 256 quantize: {c:?}"
        );
    }
    // Truecolor is a no-op.
    let mut q = ThemeName::DefaultDark.palette();
    let before = q.accent;
    q.quantize_for(ColorMode::TrueColor);
    assert_eq!(q.accent, before);
}

#[test]
fn thinking_is_not_success_after_quantize() {
    let mut p = ThemeName::DefaultDark.palette();
    p.quantize_for(ColorMode::Ansi256);
    assert_ne!(
        p.thinking, p.success,
        "thinking must stay distinct from build-green"
    );
    assert_ne!(p.dim, p.success);
}

#[test]
fn theme_parse_and_syntax_cover_all() {
    for theme in ThemeName::ALL {
        assert_eq!(theme.name().parse::<ThemeName>().unwrap(), *theme);
        let _ = theme.syntax_theme();
        let _ = theme.palette();
    }
    let err = "nope".parse::<ThemeName>().unwrap_err();
    assert!(err.to_string().contains("unknown theme"));
    ThemeName::DefaultDark.apply_syntax_theme();
    ThemeName::DefaultLight.apply_syntax_theme();
    ThemeName::TokyoNight.apply_syntax_theme();
    assert_eq!("dark".parse::<ThemeName>().unwrap(), ThemeName::DefaultDark);
    assert_eq!(
        "light".parse::<ThemeName>().unwrap(),
        ThemeName::DefaultLight
    );
    assert_eq!("onedark".parse::<ThemeName>().unwrap(), ThemeName::OneDark);
    assert_eq!(
        "tokyonight".parse::<ThemeName>().unwrap(),
        ThemeName::TokyoNight
    );
    assert_eq!(
        "tokyonightstorm".parse::<ThemeName>().unwrap(),
        ThemeName::TokyoNightStorm
    );
    assert_eq!(
        "tokyonightlight".parse::<ThemeName>().unwrap(),
        ThemeName::TokyoNightLight
    );
    assert_eq!(
        "rosepine".parse::<ThemeName>().unwrap(),
        ThemeName::RosePine
    );
    assert_eq!(
        "rosepinemoon".parse::<ThemeName>().unwrap(),
        ThemeName::RosePineMoon
    );
    assert_eq!(
        "rosepinedawn".parse::<ThemeName>().unwrap(),
        ThemeName::RosePineDawn
    );
    assert_eq!(
        "oceanicnext".parse::<ThemeName>().unwrap(),
        ThemeName::OceanicNext
    );
    assert_eq!(
        "palenight".parse::<ThemeName>().unwrap(),
        ThemeName::MaterialPalenight
    );
}

#[test]
fn extra_colors_quantize_and_palette_washes() {
    let mut extra = ExtraColors {
        agent_build: Some(Color::Rgb(0x11, 0xaa, 0x22)),
        agent_plan: Some(Color::Rgb(0x33, 0x44, 0xaa)),
        agent_ask: Some(Color::Rgb(0xaa, 0x33, 0x44)),
        model: Some(Color::Rgb(0xab, 0xcd, 0xef)),
    };
    extra.quantize_for(ColorMode::TrueColor);
    assert!(matches!(extra.agent_build, Some(Color::Rgb(_, _, _))));
    extra.quantize_for(ColorMode::Ansi256);
    assert!(!matches!(extra.agent_build, Some(Color::Rgb(_, _, _))));

    let dark = ThemeName::DefaultDark.palette();
    let _ = dark.callout_bg(dark.accent);
    let _ = dark.diff_line_bg(dark.diff_add);
    let unselected = dark.prompt_band(false);
    let selected = dark.prompt_band(true);
    assert_ne!(unselected, selected);
    let light = ThemeName::DefaultLight.palette();
    let _ = light.prompt_band(false);
    let _ = light.prompt_band(true);
    let _ = to_rgb(Color::Reset);
    let _ = to_rgb(Color::Indexed(0));
    let _ = to_rgb(Color::Indexed(16));
    let _ = to_rgb(Color::Indexed(232));
    for c in [
        Color::Black,
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::Gray,
        Color::DarkGray,
        Color::LightRed,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightBlue,
        Color::LightMagenta,
        Color::LightCyan,
        Color::White,
    ] {
        let _ = to_rgb(c);
    }
    for i in 1..=14u8 {
        let _ = to_rgb(Color::Indexed(i));
    }
    assert_eq!(dark.parse_spec("secondary"), Some(dark.user_msg));
    assert_eq!(dark.parse_spec("warning"), Some(dark.warning));
    assert_eq!(dark.parse_spec("error"), Some(dark.error));
    assert_eq!(dark.parse_spec("info"), Some(dark.info));
    assert_eq!(dark.parse_spec("thinking"), Some(dark.thinking));
    assert_eq!(dark.parse_spec("muted"), Some(dark.dim));
    assert_eq!(dark.parse_spec("highlight"), Some(dark.highlight));
    assert_eq!(dark.parse_spec("green"), Some(Color::Green));
    assert_eq!(dark.parse_spec("yellow"), Some(Color::Yellow));
    assert_eq!(dark.parse_spec("blue"), Some(Color::Blue));
    assert_eq!(dark.parse_spec("purple"), Some(Color::Magenta));
    assert_eq!(dark.parse_spec("cyan"), Some(Color::Cyan));
    assert_eq!(dark.parse_spec("white"), Some(Color::White));
    assert_eq!(dark.parse_spec("black"), Some(Color::Black));
    assert_eq!(parse_hex_color("#xyz"), None);
}
