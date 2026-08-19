// ── config.rs: TUI-specific configuration ─────────────────────────────

use crate::theme::{ExtraColors, ThemeName, ThemePalette};
use crate::theme_file;
use ratatui::style::Color;
use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use whycode_config::TuiConfig;

/// Directory, relative to the config directory, holding user theme files.
pub const THEMES_DIR: &str = "themes";

/// Configuration consumed by the TUI application.
#[derive(Debug, Clone)]
pub struct TuiAppConfig {
    /// Active theme name.
    pub theme: ThemeName,
    /// Palette loaded from a theme file, when the configured name matched one.
    /// Takes precedence over `theme`, which stays as the fallback so every
    /// existing caller keeps working.
    pub theme_override: Option<ThemePalette>,
    /// Custom keybindings from user config.
    pub key_bindings: std::collections::HashMap<String, String>,
    /// Auto-scroll on new messages.
    pub auto_scroll: bool,
    /// Show sidebar by default.
    pub show_sidebar: bool,
    /// Base scrollback limit.
    pub scrollback: usize,
    /// `[tui.agent_colors]` specs (`build = "#7aa2f7"` / `"accent"`).
    pub agent_color_specs: HashMap<String, String>,
    /// Optional prompt-chrome colors from the active theme file.
    pub extra: ExtraColors,
}

impl Default for TuiAppConfig {
    fn default() -> Self {
        Self {
            theme: ThemeName::DefaultDark,
            theme_override: None,
            key_bindings: std::collections::HashMap::new(),
            auto_scroll: true,
            show_sidebar: false,
            scrollback: 10_000,
            agent_color_specs: HashMap::new(),
            extra: ExtraColors::default(),
        }
    }
}

impl TuiAppConfig {
    /// The palette to render with.
    pub fn palette(&self) -> ThemePalette {
        self.theme_override
            .clone()
            .unwrap_or_else(|| self.theme.palette())
    }

    /// Color for the named agent in prompt/header chrome.
    ///
    /// Precedence: `[tui.agent_colors]` spec → theme-file extra role →
    /// built-in name mapping (`build`/`plan`/`ask`) → cycle index.
    pub fn agent_color(&self, name: &str, idx: usize, palette: &ThemePalette) -> Color {
        if let Some(spec) = self.agent_color_specs.get(name)
            && let Some(c) = palette.parse_spec(spec)
        {
            return c;
        }
        let themed = match name {
            "build" => self.extra.agent_build,
            "plan" => self.extra.agent_plan,
            "ask" => self.extra.agent_ask,
            _ => None,
        };
        themed.unwrap_or_else(|| palette.color_for_agent_name(name, idx))
    }

    /// Color for the provider/model caption on the prompt footer.
    pub fn model_color(&self, palette: &ThemePalette) -> Color {
        if let Some(spec) = self.agent_color_specs.get("model")
            && let Some(c) = palette.parse_spec(spec)
        {
            return c;
        }
        self.extra.model.unwrap_or(palette.info)
    }

    /// Build from the core `TuiConfig` loaded from config.toml.
    pub fn from_core_config(cfg: &TuiConfig) -> Self {
        Self::from_core_config_with_themes(cfg, whycode_config::Config::default_path().ok())
    }

    /// [`Self::from_core_config`] with the config file location supplied, so
    /// tests can point at a temporary directory.
    pub fn from_core_config_with_themes(
        cfg: &TuiConfig,
        config_path: Option<std::path::PathBuf>,
    ) -> Self {
        let mut c = Self::default();

        let themes_dir = config_path
            .as_deref()
            .and_then(Path::parent)
            .map(|d| d.join(THEMES_DIR));

        if let Some(ref theme_name) = cfg.theme {
            // A file theme wins over a built-in of the same name: the user put
            // the file there deliberately.
            if let Some(dir) = themes_dir {
                let (loaded, errors) = theme_file::load_dir(&dir);
                for (path, err) in errors {
                    tracing::warn!(path = %path.display(), "{err}");
                }
                if let Some(found) = loaded.into_iter().find(|t| &t.name == theme_name) {
                    c.theme_override = Some(found.palette);
                    c.extra = found.extra;
                }
            }

            if c.theme_override.is_none() {
                c.theme = ThemeName::from_str(theme_name).unwrap_or_else(|e| {
                    tracing::warn!("{}", e);
                    ThemeName::DefaultDark
                });
            }
        }

        if let Some(ref kb) = cfg.key_bindings {
            c.key_bindings = kb.clone();
        }
        c.show_sidebar = cfg.show_sidebar;
        c.agent_color_specs = cfg.agent_colors.clone();
        c
    }
}

#[cfg(test)]
mod tests {
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
            "whycode-cfg-{}",
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
}
