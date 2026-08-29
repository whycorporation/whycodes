//! User-visible WhyCodes directories.
//!
//! `WHYCODES_HOME` (if set and non-empty) is the instance root: config is
//! `$WHYCODES_HOME/config.toml` and session/auth/memory/browser data live
//! under `$WHYCODES_HOME`. Otherwise the XDG/platform project dirs are used.
//!
//! Project-local state lives under `.whycodes/`.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

const QUALIFIER: &str = "com";
const ORG: &str = "whycorporation";
const APP: &str = "whycodes";
const PROJECT_DIR: &str = ".whycodes";

/// Isolated instance root from the environment.
pub fn whycodes_home() -> Option<PathBuf> {
    let raw = std::env::var_os("WHYCODES_HOME")?;
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

/// Project-local WhyCodes directory: `.whycodes`.
pub fn project_dir(working_dir: &Path) -> PathBuf {
    working_dir.join(PROJECT_DIR)
}

/// Format a path for humans (status bar, toasts, copy-to-clipboard).
///
/// `std::fs::canonicalize` on Windows returns a Win32 extended-length path
/// (`\\?\C:\…` or `\\?\UNC\server\share`). Those prefixes are correct for
/// filesystem APIs and long-path support, but look wrong in the TUI. Other
/// platforms (and already-normal Windows paths) are a no-op.
pub fn display_path(path: &Path) -> String {
    strip_windows_verbatim_prefix(&path.to_string_lossy()).into_owned()
}

/// Strip `\\?\` / `\\?\UNC\` when the remainder is a drive or UNC path.
/// Device namespace paths (`\\?\pipe\…`, `\\?\Volume{guid}\…`) are left alone.
pub(crate) fn strip_windows_verbatim_prefix(s: &str) -> Cow<'_, str> {
    const VERBATIM: &str = r"\\?\";
    const UNC: &str = r"UNC\";
    let Some(rest) = s.strip_prefix(VERBATIM) else {
        return Cow::Borrowed(s);
    };
    if let Some(unc) = rest.strip_prefix(UNC) {
        return Cow::Owned(format!(r"\\{unc}"));
    }
    let b = rest.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return Cow::Borrowed(rest);
    }
    Cow::Borrowed(s)
}
