//! Themes loaded from JSON.
//!
//! Built-in palettes in [`crate::theme`] ship compiled-in. This module is the
//! override layer: theme files under the config directory use the community
//! [theme JSON schema](https://opencode.ai/theme.json) so existing files load
//! unmodified.
//!
//! The schema has two levels. `defs` names colours, and `theme` assigns those
//! names to roles, once for a dark terminal and once for a light one:
//!
//! ```json
//! {
//!   "defs":  { "darkRed": "#e06c75" },
//!   "theme": { "error": { "dark": "darkRed", "light": "#d1383d" } }
//! }
//! ```
//!
//! A role may reference a def or give a hex literal directly. Because each file
//! carries both variants, `catppuccin.json` provides two selectable themes:
//! `catppuccin` and `catppuccin-light`.

use ratatui::style::Color;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::theme::{ExtraColors, ThemeName, ThemePalette, parse_hex_color};

/// Suffix appended to the light variant of a loaded file.
pub const LIGHT_SUFFIX: &str = "-light";

#[derive(Debug, Deserialize)]
pub struct ThemeFile {
    #[serde(default)]
    pub defs: HashMap<String, String>,
    pub theme: HashMap<String, RoleValue>,
}

/// A role is either one colour for both variants, or one per variant.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum RoleValue {
    Both(String),
    PerVariant { dark: String, light: String },
}

impl RoleValue {
    fn for_variant(&self, light: bool) -> &str {
        match self {
            Self::Both(v) => v,
            Self::PerVariant { dark, light: l } => {
                if light {
                    l
                } else {
                    dark
                }
            }
        }
    }
}

/// Why a theme file could not be used. Names the role at fault, because
/// "invalid theme" gives the author nothing to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeFileError {
    Parse(String),
    UnknownDef { role: String, name: String },
    BadColor { role: String, value: String },
    MissingRole(&'static str),
}

impl std::fmt::Display for ThemeFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "could not parse theme: {e}"),
            Self::UnknownDef { role, name } => {
                write!(f, "role '{role}' references undefined colour '{name}'")
            }
            Self::BadColor { role, value } => {
                write!(f, "role '{role}' has invalid colour '{value}'")
            }
            Self::MissingRole(role) => write!(f, "theme is missing the required role '{role}'"),
        }
    }
}

impl std::error::Error for ThemeFileError {}

impl ThemeFile {
    pub fn parse(json: &str) -> Result<Self, ThemeFileError> {
        serde_json::from_str(json).map_err(|e| ThemeFileError::Parse(e.to_string()))
    }

    /// Resolve one role, following a def reference if that is what it holds.
    fn color(&self, role: &str, light: bool) -> Result<Option<Color>, ThemeFileError> {
        let Some(value) = self.theme.get(role) else {
            return Ok(None);
        };
        let raw = value.for_variant(light);

        if let Some(color) = parse_hex_color(raw) {
            return Ok(Some(color));
        }
        let Some(def) = self.defs.get(raw) else {
            return Err(ThemeFileError::UnknownDef {
                role: role.to_string(),
                name: raw.to_string(),
            });
        };
        parse_hex_color(def)
            .map(Some)
            .ok_or(ThemeFileError::BadColor {
                role: role.to_string(),
                value: def.clone(),
            })
    }

    /// Optional prompt-chrome roles. Missing keys stay `None`; a bad value
    /// is ignored so a typo on `agentBuild` does not reject the whole theme.
    pub fn extra(&self, light: bool) -> ExtraColors {
        let opt = |role: &str| self.color(role, light).ok().flatten();
        ExtraColors {
            agent_build: opt("agentBuild"),
            agent_plan: opt("agentPlan"),
            agent_ask: opt("agentAsk"),
            model: opt("model"),
        }
    }

    /// Build a palette for one variant.
    ///
    /// whycode's palette has 27 fields against the schema's 49 roles, so
    /// several roles go unused and several fields are derived from a related
    /// role. Only the roles listed in `REQUIRED` must be present; everything
    /// else falls back to the built-in dark or light theme, which keeps a
    /// partial theme file usable rather than rejecting it.
    pub fn palette(&self, light: bool) -> Result<ThemePalette, ThemeFileError> {
        const REQUIRED: &[&str] = &["background", "text", "border", "accent"];
        for role in REQUIRED {
            if !self.theme.contains_key(*role) {
                return Err(ThemeFileError::MissingRole(role));
            }
        }

        let base = if light {
            ThemeName::DefaultLight.palette()
        } else {
            ThemeName::DefaultDark.palette()
        };

        // `get` resolves a role, falling back to the built-in palette so a
        // theme that only defines the common roles still works.
        macro_rules! get {
            ($role:literal, $fallback:expr) => {
                self.color($role, light)?.unwrap_or($fallback)
            };
        }

        let text = get!("text", base.fg);
        let muted = get!("textMuted", base.dim);
        let panel = get!("backgroundPanel", base.sidebar_bg);
        let border = get!("border", base.border);

        Ok(ThemePalette {
            bg: get!("background", base.bg),
            fg: text,
            border,
            border_focused: get!("borderActive", base.border_focused),
            accent: get!("accent", base.accent),
            user_msg: get!("secondary", base.user_msg),
            assistant_msg: text,
            system_msg: muted,
            tool_msg: get!("info", base.tool_msg),
            thinking: get!("syntaxComment", muted),
            error: get!("error", base.error),
            warning: get!("warning", base.warning),
            success: get!("success", base.success),
            info: get!("info", base.info),
            dim: muted,
            highlight: get!("primary", base.highlight),
            status_bar_bg: panel,
            status_bar_fg: text,
            input_bg: get!("backgroundElement", base.input_bg),
            input_fg: text,
            sidebar_bg: panel,
            dialog_bg: panel,
            dialog_border: border,
            scrollbar: get!("borderSubtle", base.scrollbar),
            diff_add: get!("diffAdded", base.diff_add),
            diff_remove: get!("diffRemoved", base.diff_remove),
            diff_hunk: get!("diffHunkHeader", base.diff_hunk),
        })
    }
}

/// A theme loaded from disk, keyed by the name used to select it.
#[derive(Debug, Clone)]
pub struct LoadedTheme {
    pub name: String,
    pub palette: ThemePalette,
    pub extra: ExtraColors,
}

/// Read every `*.json` in `dir`, producing a dark and a light theme per file.
///
/// A file that fails to load is reported and skipped; one bad theme must not
/// stop the others from loading, and it must not stop the TUI from starting.
pub fn load_dir(dir: &Path) -> (Vec<LoadedTheme>, Vec<(PathBuf, ThemeFileError)>) {
    let mut loaded = Vec::new();
    let mut errors = Vec::new();

    let Ok(entries) = std::fs::read_dir(dir) else {
        return (loaded, errors);
    };

    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();

    for path in paths {
        let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().to_string()) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };

        match ThemeFile::parse(&text) {
            Err(e) => errors.push((path, e)),
            Ok(file) => {
                for (light, suffix) in [(false, ""), (true, LIGHT_SUFFIX)] {
                    match file.palette(light) {
                        Ok(palette) => loaded.push(LoadedTheme {
                            name: format!("{stem}{suffix}"),
                            extra: file.extra(light),
                            palette,
                        }),
                        Err(e) => errors.push((path.clone(), e)),
                    }
                }
            }
        }
    }

    (loaded, errors)
}

#[cfg(test)]
mod tests {
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
        let dir = std::env::temp_dir().join(format!("whycode-themes-{}", uuid_like()));
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
}
