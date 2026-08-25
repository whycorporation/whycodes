//! User-visible WhyCodes directories.
//!
//! `WHYCODES_HOME` (if set and non-empty) is the instance root: config is
//! `$WHYCODES_HOME/config.toml` and session/auth/memory/browser data live
//! under `$WHYCODES_HOME`. Otherwise the XDG/platform project dirs are used.
//!
//! Project-local state lives under `.whycodes/` (legacy `.whycode/` is still
//! read when the new directory is absent).

use std::path::{Path, PathBuf};

const QUALIFIER: &str = "com";
const ORG: &str = "whycorporation";
const APP: &str = "whycodes";
const PROJECT_DIR: &str = ".whycodes";
const PROJECT_DIR_LEGACY: &str = ".whycode";

/// Isolated instance root from the environment.
pub fn whycodes_home() -> Option<PathBuf> {
    let raw = std::env::var_os("WHYCODES_HOME").or_else(|| std::env::var_os("WHYCODE_HOME"))?;
    if raw.is_empty() {
        None
    } else {
        Some(PathBuf::from(raw))
    }
}

/// Sessions, auth store, memory banks, browser profile.
pub fn data_dir() -> PathBuf {
    if let Some(home) = whycodes_home() {
        return home;
    }
    or_dot(
        directories::ProjectDirs::from(QUALIFIER, ORG, APP)
            .map(|d| d.data_local_dir().to_path_buf()),
    )
}

/// `config.toml`, `skills/`, `plugins.toml`.
pub fn config_dir() -> PathBuf {
    if let Some(home) = whycodes_home() {
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

/// Project-local WhyCodes directory: `.whycodes`, or legacy `.whycode` if that
/// is the only one present.
pub fn project_dir(working_dir: &Path) -> PathBuf {
    let preferred = working_dir.join(PROJECT_DIR);
    if preferred.exists() {
        return preferred;
    }
    let legacy = working_dir.join(PROJECT_DIR_LEGACY);
    if legacy.exists() {
        return legacy;
    }
    preferred
}
