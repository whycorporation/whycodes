use super::*;

fn tui_config(theme: Option<&str>) -> TuiConfig {
    TuiConfig {
        theme: theme.map(str::to_string),
        ..Default::default()
    }
}

/// A theme file directory containing one file named `custom.json`.
fn temp_themes() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "whycodes-cfg-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(root.join(THEMES_DIR)).unwrap();
    std::fs::write(
        root.join(THEMES_DIR).join("custom.json"),
        r##"{"defs":{},"theme":{
            "background":{"dark":"#010203","light":"#fefdfc"},
            "text":{"dark":"#eeeeee","light":"#111111"},
            "border":{"dark":"#333333","light":"#cccccc"},
            "accent":{"dark":"#ff8800","light":"#884400"}
        }}"##,
    )
    .unwrap();
    root
}

#[test]
fn falls_back_to_a_built_in_when_no_file_matches() {
    let c = TuiAppConfig::from_core_config_with_themes(&tui_config(Some("monokai")), None);
    assert_eq!(c.theme, ThemeName::Monokai);
    assert!(c.theme_override.is_none());
    assert_eq!(c.palette().bg, ThemeName::Monokai.palette().bg);
}

#[test]
fn an_unknown_name_falls_back_to_the_default() {
    let c = TuiAppConfig::from_core_config_with_themes(&tui_config(Some("nope")), None);
    assert_eq!(c.theme, ThemeName::DefaultDark);
}

#[test]
fn a_theme_file_is_selected_by_its_file_name() {
    let root = temp_themes();
    let c = TuiAppConfig::from_core_config_with_themes(
        &tui_config(Some("custom")),
        Some(root.join("config.toml")),
    );
    let _ = std::fs::remove_dir_all(&root);

    assert!(c.theme_override.is_some(), "theme file should have matched");
    assert_eq!(c.palette().bg, ratatui::style::Color::Rgb(0x01, 0x02, 0x03));
}

#[test]
fn the_light_variant_is_selectable_by_suffix() {
    let root = temp_themes();
    let c = TuiAppConfig::from_core_config_with_themes(
        &tui_config(Some("custom-light")),
        Some(root.join("config.toml")),
    );
    let _ = std::fs::remove_dir_all(&root);

    assert_eq!(c.palette().bg, ratatui::style::Color::Rgb(0xfe, 0xfd, 0xfc));
}

#[test]
fn a_built_in_still_resolves_when_a_themes_directory_exists() {
    let root = temp_themes();
    let c = TuiAppConfig::from_core_config_with_themes(
        &tui_config(Some("nord")),
        Some(root.join("config.toml")),
    );
    let _ = std::fs::remove_dir_all(&root);

    assert_eq!(c.theme, ThemeName::Nord);
    assert!(c.theme_override.is_none());
}

#[test]
fn no_theme_configured_leaves_the_default() {
    let c = TuiAppConfig::from_core_config_with_themes(&tui_config(None), None);
    assert_eq!(c.theme, ThemeName::DefaultDark);
    assert!(c.theme_override.is_none());
}

#[test]
fn agent_color_specs_come_from_core_config() {
    let mut cfg = tui_config(None);
    cfg.agent_colors.insert("build".into(), "#112233".into());
    let c = TuiAppConfig::from_core_config_with_themes(&cfg, None);
    let palette = c.palette();
    assert_eq!(
        c.agent_color("build", 0, &palette),
        ratatui::style::Color::Rgb(0x11, 0x22, 0x33)
    );
}

#[test]
fn key_bindings_and_broken_theme_files_are_loaded() {
    let mut cfg = tui_config(Some("custom"));
    cfg.key_bindings = Some([("ctrl-k".into(), "kill".into())].into_iter().collect());
    let root = temp_themes();
    std::fs::write(root.join(THEMES_DIR).join("broken.json"), "{not json").unwrap();
    let c = TuiAppConfig::from_core_config_with_themes(&cfg, Some(root.join("config.toml")));
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(
        c.key_bindings.get("ctrl-k").map(String::as_str),
        Some("kill")
    );
    assert!(c.theme_override.is_some());
}
