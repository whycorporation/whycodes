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

#[test]
fn write_atomic_persist_fails_when_target_is_a_directory() {
    let dir = tempfile::TempDir::new().unwrap();
    let target = dir.path().join("blocked");
    fs::create_dir(&target).unwrap();
    let err = write_atomic(&target, "nope").unwrap_err();
    assert!(
        err.kind() == std::io::ErrorKind::AlreadyExists
            || err.kind() == std::io::ErrorKind::PermissionDenied
            || !err.to_string().is_empty()
    );
    assert!(target.is_dir());
}

#[test]
fn write_atomic_fails_when_parent_is_a_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let parent = dir.path().join("not-a-dir");
    fs::write(&parent, "x").unwrap();
    let err = write_atomic(&parent.join("child.txt"), "nope").unwrap_err();
    assert!(!err.to_string().is_empty());
}

#[cfg(unix)]
#[test]
fn write_atomic_keeps_mode_and_gives_new_files_the_umask_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::TempDir::new().unwrap();
    let script = dir.path().join("run.sh");
    fs::write(&script, "old").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o751)).unwrap();
    write_atomic(&script, "new").unwrap();
    let mode = fs::metadata(&script).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o751);

    let plain = dir.path().join("plain.txt");
    fs::write(&plain, "x").unwrap();
    let fresh = dir.path().join("fresh.txt");
    write_atomic(&fresh, "x").unwrap();
    let mode_of = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode_of(&fresh), mode_of(&plain));
}

#[cfg(unix)]
#[test]
fn write_atomic_writes_through_a_symlink() {
    let dir = tempfile::TempDir::new().unwrap();
    let real = dir.path().join("real.txt");
    fs::write(&real, "old").unwrap();
    let link = dir.path().join("link.txt");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    write_atomic(&link, "new").unwrap();
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(&real).unwrap(), "new");

    // A dangling link has nothing to write through to.
    let dangling = dir.path().join("dangling");
    std::os::unix::fs::symlink(dir.path().join("missing"), &dangling).unwrap();
    assert!(write_atomic(&dangling, "x").is_err());
}

#[cfg(unix)]
#[test]
fn write_atomic_reports_an_unreadable_target() {
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join("f");
    fs::write(&file, "x").unwrap();
    // `f/child` fails to stat with NotADirectory, not NotFound.
    assert!(write_atomic(&file.join("child"), "x").is_err());
}
