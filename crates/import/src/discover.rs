//! Locate other agents' user-level settings files without reading them.

use std::path::{Path, PathBuf};

use crate::consent::ConsentStore;
use crate::types::{FoundSource, Product, SourceState};

/// A settings file another CLI is known to write.
#[derive(Debug, Clone, Copy)]
pub struct KnownSource {
    pub product: Product,
    pub rel_path: &'static str,
}

/// Fixed relative-to-home locations. OpenCode also checks `$XDG_CONFIG_HOME`.
pub const KNOWN_SOURCES: &[KnownSource] = &[
    KnownSource {
        product: Product::Claude,
        rel_path: ".claude.json",
    },
    KnownSource {
        product: Product::Claude,
        rel_path: ".claude/settings.json",
    },
    KnownSource {
        product: Product::Claude,
        rel_path: ".claude/mcp.json",
    },
    KnownSource {
        product: Product::OpenCode,
        rel_path: ".config/opencode/opencode.json",
    },
    KnownSource {
        product: Product::OpenCode,
        rel_path: ".config/opencode/opencode.jsonc",
    },
    KnownSource {
        product: Product::Grok,
        rel_path: ".grok/config.toml",
    },
    KnownSource {
        product: Product::Codex,
        rel_path: ".codex/config.toml",
    },
];

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|h| !h.is_empty()))
        .map(PathBuf::from)
}

/// Scan `$HOME` for known settings files.
pub fn scan(consent: &ConsentStore) -> Vec<FoundSource> {
    match home_dir() {
        Some(home) => scan_with_home(&home, consent),
        None => Vec::new(),
    }
}

pub fn scan_with_home(home: &Path, consent: &ConsentStore) -> Vec<FoundSource> {
    let mut found = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for source in KNOWN_SOURCES {
        push_if_present(
            &mut found,
            &mut seen,
            home.join(source.rel_path),
            source.product,
            source.rel_path,
            consent,
        );
    }
    for path in extra_opencode_paths(home) {
        push_if_present(
            &mut found,
            &mut seen,
            path,
            Product::OpenCode,
            ".config/opencode/opencode.json",
            consent,
        );
    }
    found
}

fn extra_opencode_paths(home: &Path) -> Vec<PathBuf> {
    extra_opencode_paths_with(home, std::env::var_os("XDG_CONFIG_HOME"))
}

fn extra_opencode_paths_with(home: &Path, xdg: Option<std::ffi::OsString>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(xdg) = xdg.filter(|h| !h.is_empty()) {
        let base = PathBuf::from(xdg).join("opencode");
        out.push(base.join("opencode.json"));
        out.push(base.join("opencode.jsonc"));
    }
    // Windows-ish fallback; harmless if missing.
    out.push(home.join("AppData/Roaming/opencode/opencode.json"));
    out
}

fn push_if_present(
    found: &mut Vec<FoundSource>,
    seen: &mut std::collections::BTreeSet<PathBuf>,
    path: PathBuf,
    product: Product,
    rel_path: &'static str,
    consent: &ConsentStore,
) {
    if !path.exists() || !seen.insert(path.clone()) {
        return;
    }
    let state = if is_symlink(&path) {
        SourceState::Symlink
    } else {
        consent.state_for(&path)
    };
    found.push(FoundSource {
        product,
        rel_path,
        path,
        state,
    });
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// True when WhyCodes has no user `config.toml` yet (first-run candidate).
pub fn why_config_missing() -> bool {
    let path = whycodes_core::paths::config_file();
    !path.exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_finds_files_skips_missing() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(home.join(".claude.json"), "{}").unwrap();
        std::fs::write(home.join(".claude/mcp.json"), "{}").unwrap();
        std::fs::create_dir_all(home.join(".grok")).unwrap();
        std::fs::write(home.join(".grok/config.toml"), "[mcp_servers]\n").unwrap();
        let consent = ConsentStore::new(home.join("data"));
        let found = scan_with_home(home, &consent);
        assert!(
            found
                .iter()
                .any(|f| f.product == Product::Claude && f.rel_path == ".claude.json")
        );
        assert!(found.iter().any(|f| f.product == Product::Grok));
        assert!(found.iter().all(|f| f.state == SourceState::New));
        assert!(!found.iter().any(|f| f.product == Product::Codex));
    }

    #[test]
    fn scan_marks_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let real = home.join("real.json");
        std::fs::write(&real, "{}").unwrap();
        let link = home.join(".claude.json");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&real, &link).unwrap();
            let consent = ConsentStore::new(home.join("data"));
            let found = scan_with_home(home, &consent);
            let f = found.iter().find(|f| f.rel_path == ".claude.json").unwrap();
            assert_eq!(f.state, SourceState::Symlink);
        }
        #[cfg(not(unix))]
        {
            let _ = (real, link);
        }
    }

    #[test]
    fn extra_opencode_from_xdg() {
        let dir = tempfile::tempdir().unwrap();
        let xdg = dir.path().join("xdg");
        std::fs::create_dir_all(xdg.join("opencode")).unwrap();
        std::fs::write(xdg.join("opencode/opencode.json"), "{}").unwrap();
        let extra = extra_opencode_paths_with(dir.path(), Some(xdg.clone().into_os_string()));
        assert!(extra.iter().any(|p| p.ends_with("opencode.json")));
        let consent = ConsentStore::new(dir.path().join("data"));
        // scan_with_home only sees XDG via env; push the extra path as if it existed under home.
        std::fs::create_dir_all(dir.path().join(".config/opencode")).unwrap();
        std::fs::write(dir.path().join(".config/opencode/opencode.json"), "{}").unwrap();
        let found = scan_with_home(dir.path(), &consent);
        assert!(
            found
                .iter()
                .any(|f| f.product == Product::OpenCode && f.path.ends_with("opencode.json"))
        );
        let _ = extra;
    }

    #[test]
    fn scan_and_home_dir() {
        let _ = scan(&ConsentStore::new(tempfile::tempdir().unwrap().path()));
        let _ = home_dir();
        let _ = why_config_missing();
        assert!(!KNOWN_SOURCES.is_empty());
        assert_eq!(KNOWN_SOURCES[0].product, Product::Claude);
        let extra = extra_opencode_paths_with(std::path::Path::new("/tmp"), None);
        assert!(extra.iter().any(|p| p.ends_with("opencode.json")));
        let _ = extra_opencode_paths(std::path::Path::new("/tmp"));
    }

    #[test]
    fn scan_empty_without_files() {
        let dir = tempfile::tempdir().unwrap();
        let consent = ConsentStore::new(dir.path().join("data"));
        assert!(scan_with_home(dir.path(), &consent).is_empty());
    }
}
