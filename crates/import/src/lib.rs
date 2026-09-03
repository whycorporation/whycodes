//! Import MCP, permission, and hook settings from other coding agents.
//!
//! Discovery never reads a file until the user approves that exact path.
//! Source files are never modified. Symlinks are refused.

pub mod apply;
pub mod consent;
pub mod discover;
pub mod error;
pub mod extract;
pub mod parse;
pub mod types;

pub use apply::{apply, apply_and_save, plan};
pub use consent::ConsentStore;
pub use discover::{KNOWN_SOURCES, scan, scan_with_home, why_config_missing};
pub use error::{ImportError, Result};
pub use extract::extract;
pub use types::{
    Extracted, FoundSource, ImportItem, ImportItemKind, ImportPlan, Product, SourceState,
};

/// Read approved sources and build a merge plan against `config`.
pub fn preview(
    sources: &[FoundSource],
    config: &whycodes_config::Config,
    force: bool,
) -> Result<(Vec<Extracted>, ImportPlan)> {
    let mut extracted = Vec::new();
    for src in sources {
        match src.state {
            SourceState::Approved => extracted.push(extract(src)?),
            SourceState::New | SourceState::Denied | SourceState::Symlink => {}
        }
    }
    let plan = plan(config, &extracted, force);
    Ok((extracted, plan))
}

/// True when at least one non-symlink foreign settings file exists.
pub fn has_discoverable_sources(home: &std::path::Path, consent: &ConsentStore) -> bool {
    scan_with_home(home, consent)
        .iter()
        .any(|s| s.state != SourceState::Symlink)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn preview_skips_unapproved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".claude.json");
        std::fs::write(&path, r#"{"mcpServers":{"fs":{"command":"npx"}}}"#).unwrap();
        let src = FoundSource {
            product: Product::Claude,
            rel_path: ".claude.json",
            path: path.clone(),
            state: SourceState::New,
        };
        let cfg = whycodes_config::Config::default();
        let (ex, plan) = preview(&[src], &cfg, false).unwrap();
        assert!(ex.is_empty());
        assert!(plan.is_empty());
        let src = FoundSource {
            product: Product::Claude,
            rel_path: ".claude.json",
            path,
            state: SourceState::Approved,
        };
        let (ex, plan) = preview(&[src], &cfg, false).unwrap();
        assert_eq!(ex.len(), 1);
        assert_eq!(plan.mcp_add.len(), 1);
    }

    #[test]
    fn has_discoverable_sources_true() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".claude.json"), "{}").unwrap();
        let consent = ConsentStore::new(dir.path().join("data"));
        assert!(has_discoverable_sources(dir.path(), &consent));
        assert!(!has_discoverable_sources(
            &PathBuf::from("/no-such-home-xyz"),
            &consent
        ));
    }
}
