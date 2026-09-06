//! Same-directory temp file + rename so a crash never leaves a half-written
//! target. Callers already hold the swarm file claim.

use std::io::Write;
use std::path::Path;

/// Write `contents` to `path` via a sibling tempfile that is renamed into place.
pub fn write_atomic(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(contents.as_ref())?;
    tmp.flush()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
#[path = "atomic_tests.rs"]
mod tests;
