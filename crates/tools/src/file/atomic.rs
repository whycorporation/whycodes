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
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn write_atomic_replaces_and_creates() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("f.txt");
        write_atomic(&path, "one").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "one");
        write_atomic(&path, "two").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "two");
    }

    #[test]
    fn write_atomic_empty_parent_uses_dot() {
        let dir = tempfile::TempDir::new().unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        write_atomic(Path::new("rel.txt"), "x").unwrap();
        assert_eq!(fs::read_to_string("rel.txt").unwrap(), "x");
        std::env::set_current_dir(prev).unwrap();
    }
}
