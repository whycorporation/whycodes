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
