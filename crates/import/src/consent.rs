//! Consent store for settings import (`<data_dir>/import-consent.json`, 0600).
//!
//! Approvals are per canonical path. The source file is never modified.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::types::SourceState;

const VERSION: u32 = 1;

#[derive(Debug, Default, Serialize, Deserialize)]
struct ConsentFile {
    #[serde(default)]
    version: u32,
    /// First-run prompt already shown (yes or no).
    #[serde(default)]
    first_run_asked: bool,
    #[serde(default)]
    approved: BTreeSet<String>,
    #[serde(default)]
    denied: BTreeSet<String>,
}

pub struct ConsentStore {
    path: PathBuf,
}

impl ConsentStore {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            path: data_dir.into().join("import-consent.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read(&self) -> Result<ConsentFile> {
        if !self.path.exists() {
            return Ok(ConsentFile {
                version: VERSION,
                ..ConsentFile::default()
            });
        }
        let bytes = std::fs::read(&self.path)?;
        if bytes.is_empty() {
            return Ok(ConsentFile {
                version: VERSION,
                ..ConsentFile::default()
            });
        }
        let mut file: ConsentFile = serde_json::from_slice(&bytes)?;
        file.version = VERSION;
        Ok(file)
    }

    fn write(&self, file: &ConsentFile) -> Result<()> {
        let parent = self.path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(file)?)?;
        set_owner_only(&tmp)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub fn first_run_asked(&self) -> Result<bool> {
        Ok(self.read()?.first_run_asked)
    }

    pub fn mark_first_run_asked(&self) -> Result<()> {
        let mut file = self.read()?;
        file.first_run_asked = true;
        self.write(&file)
    }

    pub fn state_for(&self, path: &Path) -> SourceState {
        let key = path_key(path);
        match self.read() {
            Ok(file) if file.denied.contains(&key) => SourceState::Denied,
            Ok(file) if file.approved.contains(&key) => SourceState::Approved,
            _ => SourceState::New,
        }
    }

    pub fn approve(&self, path: &Path) -> Result<()> {
        let key = path_key(path);
        let mut file = self.read()?;
        file.denied.remove(&key);
        file.approved.insert(key);
        self.write(&file)
    }

    pub fn deny(&self, path: &Path) -> Result<()> {
        let key = path_key(path);
        let mut file = self.read()?;
        file.approved.remove(&key);
        file.denied.insert(key);
        self.write(&file)
    }
}

fn path_key(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

#[cfg(unix)]
fn set_owner_only(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approve_deny_and_first_run() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConsentStore::new(dir.path());
        let p = dir.path().join("a.json");
        std::fs::write(&p, "{}").unwrap();
        assert!(!store.first_run_asked().unwrap());
        assert_eq!(store.state_for(&p), SourceState::New);
        store.approve(&p).unwrap();
        assert_eq!(store.state_for(&p), SourceState::Approved);
        store.deny(&p).unwrap();
        assert_eq!(store.state_for(&p), SourceState::Denied);
        store.approve(&p).unwrap();
        assert_eq!(store.state_for(&p), SourceState::Approved);
        store.mark_first_run_asked().unwrap();
        assert!(store.first_run_asked().unwrap());
        assert!(store.path().ends_with("import-consent.json"));
        // empty file is treated as default
        std::fs::write(store.path(), b"").unwrap();
        assert!(!store.first_run_asked().unwrap());
    }

    #[test]
    fn missing_file_is_new() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConsentStore::new(dir.path());
        assert_eq!(
            store.state_for(&dir.path().join("nope.json")),
            SourceState::New
        );
    }

    #[test]
    fn invalid_json_and_missing_canonical_path() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConsentStore::new(dir.path());
        std::fs::write(store.path(), b"{not json").unwrap();
        assert!(store.first_run_asked().is_err());
        std::fs::remove_file(store.path()).unwrap();
        let missing = dir.path().join("no-such.json");
        assert_eq!(store.state_for(&missing), SourceState::New);
        store.approve(&missing).unwrap();
        assert_eq!(store.state_for(&missing), SourceState::Approved);
    }

    #[cfg(not(unix))]
    #[test]
    fn set_owner_only_is_noop_off_unix() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x");
        std::fs::write(&p, b"x").unwrap();
        set_owner_only(&p).unwrap();
    }
}
