// ── config.rs: TUI-specific configuration ─────────────────────────────

use whycode_core::config::TuiConfig;
use crate::theme::ThemeName;

/// Configuration consumed by the TUI application.
#[derive(Debug, Clone)]
pub struct TuiAppConfig {
    /// Active theme name.
    pub theme: ThemeName,
    /// Custom keybindings from user config.
    pub key_bindings: std::collections::HashMap<String, String>,
    /// Auto-scroll on new messages.
    pub auto_scroll: bool,
    /// Show sidebar by default.
    pub show_sidebar: bool,
    /// Base scrollback limit.
    pub scrollback: usize,
}

impl Default for TuiAppConfig {
    fn default() -> Self {
        Self {
            theme: ThemeName::DefaultDark,
            key_bindings: std::collections::HashMap::new(),
            auto_scroll: true,
            show_sidebar: false,
            scrollback: 10_000,
        }
    }
}

impl TuiAppConfig {
    /// Build from the core `TuiConfig` loaded from config.toml.
    pub fn from_core_config(cfg: &TuiConfig) -> Self {
        let mut c = Self::default();
        if let Some(ref theme_name) = cfg.theme {
            c.theme = ThemeName::from_str(theme_name);
        }
        if let Some(ref kb) = cfg.key_bindings {
            c.key_bindings = kb.clone();
        }
        c
    }
}
