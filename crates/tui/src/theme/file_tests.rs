use super::*;

/// A minimal file in the real schema shape.
const SAMPLE: &str = r##"{
    "$schema": "https://opencode.ai/theme.json",
    "defs": {
        "darkBg": "#0a0a0a",
        "darkText": "#eeeeee",
        "lightBg": "#ffffff",
        "lightText": "#1a1a1a",
        "red": "#e06c75"
    },
    "theme": {
        "background": { "dark": "darkBg",   "light": "lightBg" },
        "text":       { "dark": "darkText", "light": "lightText" },
        "border":     { "dark": "#333333",  "light": "#cccccc" },
        "accent":     "red",
        "error":      { "dark": "red",      "light": "red" }
    }
}"##;

#[test]
fn parses_the_theme_schema() {
    let file = ThemeFile::parse(SAMPLE).unwrap();
    assert_eq!(file.defs.len(), 5);
    assert!(file.theme.contains_key("background"));
}

#[test]
fn resolves_def_references_per_variant() {
    let file = ThemeFile::parse(SAMPLE).unwrap();
    assert_eq!(
        file.palette(false).unwrap().bg,
        Color::Rgb(0x0a, 0x0a, 0x0a)
    );
    assert_eq!(file.palette(true).unwrap().bg, Color::Rgb(0xff, 0xff, 0xff));
}

#[test]
fn resolves_hex_literals_given_directly_to_a_role() {
    let file = ThemeFile::parse(SAMPLE).unwrap();
    assert_eq!(
        file.palette(false).unwrap().border,
        Color::Rgb(0x33, 0x33, 0x33)
    );
}

#[test]
fn a_single_value_applies_to_both_variants() {
    let file = ThemeFile::parse(SAMPLE).unwrap();
    let red = Color::Rgb(0xe0, 0x6c, 0x75);
    assert_eq!(file.palette(false).unwrap().accent, red);
    assert_eq!(file.palette(true).unwrap().accent, red);
}

#[test]
fn extra_prompt_roles_are_optional() {
    let file = ThemeFile::parse(SAMPLE).unwrap();
    assert_eq!(file.extra(false), ExtraColors::default());

    let json = r##"{"defs":{},"theme":{
        "background":{"dark":"#000000","light":"#ffffff"},
        "text":{"dark":"#ffffff","light":"#000000"},
        "border":{"dark":"#111111","light":"#eeeeee"},
        "accent":{"dark":"#ff0000","light":"#aa0000"},
        "agentBuild":"#11aa22",
        "agentPlan":"#3344aa",
        "model":{"dark":"#abcdef","light":"#123456"}
    }}"##;
    let file = ThemeFile::parse(json).unwrap();
    let extra = file.extra(false);
    assert_eq!(extra.agent_build, Some(Color::Rgb(0x11, 0xaa, 0x22)));
    assert_eq!(extra.agent_plan, Some(Color::Rgb(0x33, 0x44, 0xaa)));
    assert_eq!(extra.agent_ask, None);
    assert_eq!(extra.model, Some(Color::Rgb(0xab, 0xcd, 0xef)));
    assert_eq!(file.extra(true).model, Some(Color::Rgb(0x12, 0x34, 0x56)));
}

#[test]
fn unspecified_roles_fall_back_to_the_built_in_palette() {
    let file = ThemeFile::parse(SAMPLE).unwrap();
    let palette = file.palette(false).unwrap();
    assert_eq!(palette.success, ThemeName::DefaultDark.palette().success);
}

#[test]
fn an_undefined_reference_names_the_role_and_the_name() {
    let json = r##"{"defs":{},"theme":{
        "background":{"dark":"#000","light":"#fff"},
        "text":{"dark":"#fff","light":"#000"},
        "border":{"dark":"#111","light":"#eee"},
        "accent":{"dark":"nosuch","light":"nosuch"}
    }}"##;
    let err = ThemeFile::parse(json).unwrap().palette(false).unwrap_err();
    assert_eq!(
        err,
        ThemeFileError::UnknownDef {
            role: "accent".into(),
            name: "nosuch".into()
        }
    );
    assert!(err.to_string().contains("accent"));
    assert!(err.to_string().contains("nosuch"));
}

#[test]
fn a_missing_required_role_is_reported_by_name() {
    let json = r##"{"defs":{},"theme":{"background":"#000"}}"##;
    let err = ThemeFile::parse(json).unwrap().palette(false).unwrap_err();
    assert!(matches!(err, ThemeFileError::MissingRole(_)));
    assert!(err.to_string().contains("missing"));
}

#[test]
fn malformed_json_reports_a_parse_error_rather_than_panicking() {
    assert!(matches!(
        ThemeFile::parse("{not json"),
        Err(ThemeFileError::Parse(_))
    ));
}

#[test]
fn parses_both_hex_lengths() {
    assert_eq!(parse_hex_color("#abc"), Some(Color::Rgb(0xaa, 0xbb, 0xcc)));
    assert_eq!(
        parse_hex_color("#aabbcc"),
        Some(Color::Rgb(0xaa, 0xbb, 0xcc))
    );
    assert_eq!(parse_hex_color("aabbcc"), None);
    assert_eq!(parse_hex_color("#gg0000"), None);
    assert_eq!(parse_hex_color("#ab"), None);
}

#[test]
fn a_missing_directory_yields_nothing_rather_than_an_error() {
    let (loaded, errors) = load_dir(Path::new("/definitely/not/here"));
    assert!(loaded.is_empty());
    assert!(errors.is_empty());
}

#[test]
fn loading_a_directory_yields_a_dark_and_a_light_theme_per_file() {
    let dir = std::env::temp_dir().join(format!("whycodes-themes-{}", uuid_like()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("sample.json"), SAMPLE).unwrap();
    std::fs::write(dir.join("broken.json"), "{not json").unwrap();
    std::fs::write(dir.join("ignored.txt"), SAMPLE).unwrap();

    let (loaded, errors) = load_dir(&dir);
    let _ = std::fs::remove_dir_all(&dir);

    let names: Vec<&str> = loaded.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["sample", "sample-light"]);
    // The broken file is reported, not silently dropped, and does not stop
    // the good one loading.
    assert_eq!(errors.len(), 1);
    assert!(errors[0].0.ends_with("broken.json"));
}

/// A unique-enough suffix without pulling in a uuid dependency.
fn uuid_like() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}
