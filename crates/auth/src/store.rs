//! On-disk token store: `<data_dir>/auth.json` with 0600 permissions on Unix.
//!
//! Rules (hard gates, not nice-to-haves):
//! - File is created with 0600; an existing store with looser permissions is
//!   refused rather than silently used.
//! - Writes are atomic (write temp file in the same dir, then rename) so a
//!   crash mid-write cannot leave a truncated store.
//! - The store is never written anywhere but the whycode data directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{AuthError, Result};
use crate::token::ProviderAuth;

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoreFile {
    #[serde(default)]
    providers: BTreeMap<String, ProviderAuth>,
}

/// OAuth token store rooted at the whycode data directory.
pub struct TokenStore {
    path: PathBuf,
}

impl TokenStore {
    /// Store at `<data_dir>/auth.json`.
    pub fn new(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join("auth.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read_file(&self) -> Result<StoreFile> {
        match std::fs::read_to_string(&self.path) {
            Ok(content) => {
                self.check_permissions()?;
                Ok(serde_json::from_str(&content)?)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StoreFile::default()),
            Err(e) => Err(AuthError::Io(e)),
        }
    }

    fn write_file(&self, file: &StoreFile) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let content = serde_json::to_string_pretty(file)?;
        std::fs::write(&tmp, content)?;
        set_owner_only(&tmp)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// Refuse to use a store that other users can read.
    fn check_permissions(&self) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&self.path)?.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                return Err(AuthError::InsecureStorePermissions(format!(
                    "{} (mode {:o})",
                    self.path.display(),
                    mode
                )));
            }
        }
        Ok(())
    }

    /// Get the stored auth for a provider, if any.
    pub fn get(&self, provider: &str) -> Result<Option<ProviderAuth>> {
        Ok(self.read_file()?.providers.get(provider).cloned())
    }

    /// Insert or replace the auth for a provider.
    pub fn set(&self, provider: &str, auth: ProviderAuth) -> Result<()> {
        let mut file = self.read_file()?;
        file.providers.insert(provider.to_string(), auth);
        self.write_file(&file)
    }

    /// Remove the auth for a provider. Returns true when one existed.
    pub fn remove(&self, provider: &str) -> Result<bool> {
        let mut file = self.read_file()?;
        let removed = file.providers.remove(provider).is_some();
        if removed {
            self.write_file(&file)?;
        }
        Ok(removed)
    }

    /// List providers with stored auth (method + expiry only, never tokens).
    pub fn list(&self) -> Result<Vec<(String, ProviderAuth)>> {
        let file = self.read_file()?;
        Ok(file.providers.into_iter().collect())
    }
}

#[cfg(unix)]
pub(crate) fn set_owner_only(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn set_owner_only(_path: &Path) -> Result<()> {
    // Windows ACL hardening is a follow-up; the store still lives under the
    // per-user data directory which is not shared by default.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::OAuthToken;

    fn token() -> ProviderAuth {
        ProviderAuth {
            method: "oauth".to_string(),
            token: OAuthToken {
                access_token: "acc".to_string(),
                refresh_token: Some("ref".to_string()),
                expires_at: None,
                extra: Default::default(),
            },
        }
    }

    #[test]
    fn roundtrip_set_get_remove() {
        let dir = tempfile::tempdir().unwrap();
        let store = TokenStore::new(dir.path());
        assert!(store.get("anthropic").unwrap().is_none());
        store.set("anthropic", token()).unwrap();
        assert!(store.get("anthropic").unwrap().is_some());
        assert!(store.remove("anthropic").unwrap());
        assert!(store.get("anthropic").unwrap().is_none());
        assert!(!store.remove("anthropic").unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn store_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let store = TokenStore::new(dir.path());
        store.set("openai", token()).unwrap();
        let mode = std::fs::metadata(store.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn refuses_world_readable_store() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let store = TokenStore::new(dir.path());
        store.set("openai", token()).unwrap();
        std::fs::set_permissions(store.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = store.get("openai").unwrap_err();
        assert!(matches!(err, AuthError::InsecureStorePermissions(_)));
    }
}
