//! User-visible whycode directories.
//!
//! `WHYCODE_HOME` (if set and non-empty) is the instance root: config is
//! `$WHYCODE_HOME/config.toml` and session/auth/memory/browser data live
//! under `$WHYCODE_HOME`. Otherwise the XDG/platform project dirs are used.

use std::path::PathBuf;

const QUALIFIER: &str = "com";
const ORG: &str = "whycorporation";
const APP: &str = "whycode";

/// Isolated instance root from the environment.
pub fn whycode_home() -> Option<PathBuf> {
    let raw = std::env::var_os("WHYCODE_HOME")?;
    if raw.is_empty() {
        None
    } else {
        Some(PathBuf::from(raw))
    }
}

/// Sessions, auth store, memory banks, browser profile.
pub fn data_dir() -> PathBuf {
    if let Some(home) = whycode_home() {
        return home;
    }
    or_dot(
        directories::ProjectDirs::from(QUALIFIER, ORG, APP)
            .map(|d| d.data_local_dir().to_path_buf()),
    )
}

/// `config.toml`, `skills/`, `plugins.toml`.
pub fn config_dir() -> PathBuf {
    if let Some(home) = whycode_home() {
        return home;
    }
    or_dot(
        directories::ProjectDirs::from(QUALIFIER, ORG, APP).map(|d| d.config_dir().to_path_buf()),
    )
}

/// `$config_dir/config.toml`.
pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

pub(crate) fn or_dot(p: Option<PathBuf>) -> PathBuf {
    p.unwrap_or_else(|| PathBuf::from("."))
}
