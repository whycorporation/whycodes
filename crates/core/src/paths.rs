//! User-visible WhyCodes directories.
//!
//! `WHYCODES_HOME` (if set and non-empty) is the instance root: config is
//! `$WHYCODES_HOME/config.toml` and session/auth/memory/browser data live
//! under `$WHYCODES_HOME`. Otherwise the default is `$HOME/.whycodes`
//! (`%USERPROFILE%\.whycodes` on Windows) — the same short shape as Claude
//! (`~/.claude`), Grok (`~/.grok`), and Codex (`~/.codex`).
//!
//! Pre-0.7 installs used `directories::ProjectDirs` (`com.whycorporation.whycodes`
//! / XDG). [`legacy_instance_roots`] lists those paths so config load can
//! copy `config.toml` / `auth.json` / `whycodes.db` into `~/.whycodes`.
//!
//! Project-local state lives under `.whycodes/` next to a repo.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

const QUALIFIER: &str = "com";
const ORG: &str = "whycorporation";
const APP: &str = "whycodes";
const PROJECT_DIR: &str = ".whycodes";
const INSTANCE_DIR: &str = ".whycodes";

/// Isolated instance root from the environment.
pub fn whycodes_home() -> Option<PathBuf> {
    let raw = std::env::var_os("WHYCODES_HOME")?;
    if raw.is_empty() {
        None
    } else {
        Some(PathBuf::from(raw))
    }
}

/// User home (`$HOME`, then `%USERPROFILE%`).
pub fn user_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()))
        .map(PathBuf::from)
}

/// Config + data live together under this directory.
pub fn instance_root() -> PathBuf {
    if let Some(home) = whycodes_home() {
        return home;
    }
    or_dot(user_home().map(|h| h.join(INSTANCE_DIR)))
}

/// Sessions, auth store, memory banks, browser profile.
pub fn data_dir() -> PathBuf {
    instance_root()
}

/// `config.toml`, `skills/`, `plugins.toml`.
pub fn config_dir() -> PathBuf {
    instance_root()
}

/// `$config_dir/config.toml`.
pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

/// Former ProjectDirs locations (config and/or data). Used only to migrate
/// an existing install into [`instance_root`].
pub fn legacy_instance_roots() -> Vec<PathBuf> {
    let Some(dirs) = directories::ProjectDirs::from(QUALIFIER, ORG, APP) else {
        return Vec::new();
    };
    unique_legacy_roots(
        dirs.config_dir().to_path_buf(),
        dirs.data_local_dir().to_path_buf(),
    )
}

pub(crate) fn unique_legacy_roots(config: PathBuf, data: PathBuf) -> Vec<PathBuf> {
    if config == data {
        vec![config]
    } else {
        vec![config, data]
    }
}

pub(crate) fn or_dot(p: Option<PathBuf>) -> PathBuf {
    p.unwrap_or_else(|| PathBuf::from("."))
}

/// Project-local WhyCodes directory: `.whycodes`.
pub fn project_dir(working_dir: &Path) -> PathBuf {
    working_dir.join(PROJECT_DIR)
}

/// Throwaway agent files (`bg-*.log`, detached output). Not the OS temp dir.
pub fn project_scratch_dir(working_dir: &Path) -> PathBuf {
    project_dir(working_dir).join("scratch")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_strip_project_dir_and_or_dot() {
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\C:\foo").as_ref(),
            r"C:\foo"
        );
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\d:\bar").as_ref(),
            r"d:\bar"
        );
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\UNC\srv\share").as_ref(),
            r"\\srv\share"
        );
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\pipe\name").as_ref(),
            r"\\?\pipe\name"
        );
        assert_eq!(strip_windows_verbatim_prefix("/tmp/a").as_ref(), "/tmp/a");
        assert_eq!(display_path(Path::new("/tmp/a")), "/tmp/a");
        assert_eq!(project_dir(Path::new("/w")), PathBuf::from("/w/.whycodes"));
        assert_eq!(or_dot(None), PathBuf::from("."));
        assert_eq!(or_dot(Some(PathBuf::from("/x"))), PathBuf::from("/x"));
        assert_eq!(config_file().file_name().unwrap(), "config.toml");
        let roots = legacy_instance_roots();
        assert!(
            roots.len() <= 2,
            "legacy roots are config and/or data: {roots:?}"
        );
        assert_eq!(
            unique_legacy_roots(PathBuf::from("/a"), PathBuf::from("/a")),
            vec![PathBuf::from("/a")]
        );
        assert_eq!(
            unique_legacy_roots(PathBuf::from("/a"), PathBuf::from("/b")),
            vec![PathBuf::from("/a"), PathBuf::from("/b")]
        );
    }
}
