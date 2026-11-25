//! Same-directory temp file + rename so a crash never leaves a half-written
//! target. Callers already hold the swarm file claim.

use std::io::Write;
use std::path::{Path, PathBuf};

fn persist_error(e: tempfile::PersistError) -> std::io::Error {
    e.error
}

/// Write `contents` to `path` via a sibling tempfile that is renamed into place.
///
/// A symlink is written through: the file it points at is replaced and the
/// link stays. An existing file keeps its permissions (a script stays
/// executable); a new one gets the usual umask-filtered mode, not the
/// tempfile's private 0600.
pub fn write_atomic(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    let target = resolve_link(path)?;
    let parent = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut tmp = temp_beside(parent, &target)?;
    tmp.write_all(contents.as_ref())?;
    tmp.flush()?;
    tmp.persist(&target).map_err(persist_error)?;
    Ok(())
}

/// The file `path` names, following a symlink so the rename replaces the
/// link's target instead of the link.
fn resolve_link(path: &Path) -> std::io::Result<PathBuf> {
    let is_link = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
    if is_link {
        std::fs::canonicalize(path)
    } else {
        Ok(path.to_path_buf())
    }
}

#[cfg(unix)]
fn temp_beside(parent: &Path, target: &Path) -> std::io::Result<tempfile::NamedTempFile> {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(target) {
        Ok(meta) => {
            let tmp = tempfile::NamedTempFile::new_in(parent)?;
            tmp.as_file().set_permissions(meta.permissions())?;
            Ok(tmp)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o666))
            .tempfile_in(parent),
        Err(e) => Err(e),
    }
}

/// Windows has no mode bits to keep, and a read-only tempfile could not be
/// cleaned up, so the default tempfile is right there.
#[cfg(not(unix))]
fn temp_beside(parent: &Path, _target: &Path) -> std::io::Result<tempfile::NamedTempFile> {
    tempfile::NamedTempFile::new_in(parent)
}

#[cfg(test)]
#[path = "atomic_tests.rs"]
mod tests;
