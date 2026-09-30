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
