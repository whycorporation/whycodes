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
    directories::ProjectDirs::from(QUALIFIER, ORG, APP)
        .map(|d| d.data_local_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `config.toml`, `skills/`, `plugins.toml`.
pub fn config_dir() -> PathBuf {
    if let Some(home) = whycode_home() {
        return home;
    }
    directories::ProjectDirs::from(QUALIFIER, ORG, APP)
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `$config_dir/config.toml`.
pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_override_wins() {
        let dir = tempfile::tempdir().expect("tempdir");
        let prev = std::env::var_os("WHYCODE_HOME");
        unsafe { std::env::set_var("WHYCODE_HOME", dir.path()) };
        let data = data_dir();
        let cfg = config_file();
        match prev {
            Some(v) => unsafe { std::env::set_var("WHYCODE_HOME", v) },
            None => unsafe { std::env::remove_var("WHYCODE_HOME") },
        }
        assert_eq!(data, dir.path());
        assert_eq!(cfg, dir.path().join("config.toml"));
    }
}
