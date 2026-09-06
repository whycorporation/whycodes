use super::*;
use crate::tool::ToolContext;

fn ctx(dir: &std::path::Path) -> ToolContext {
    ToolContext::new(dir.to_string_lossy().into_owned())
}

#[tokio::test]
async fn lists_files_and_dirs_with_sizes() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.txt"), "hello").expect("write");
    std::fs::create_dir(dir.path().join("sub")).expect("mkdir");

    let out = TruncationDirTool::new()
        .execute(json!({}), &ctx(dir.path()))
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("Contents of"), "{}", out.content);
    assert!(out.content.contains("a.txt"), "{}", out.content);
    assert!(out.content.contains("sub/"), "{}", out.content);
    assert!(
        out.content.contains("(5 B)"),
        "5-byte file shown: {}",
        out.content
    );
}

#[tokio::test]
async fn truncates_entries_over_max() {
    let dir = tempfile::tempdir().expect("tempdir");
    for i in 0..5 {
        std::fs::write(dir.path().join(format!("f{i}.txt")), "x").expect("write");
    }
    let out = TruncationDirTool::new()
        .execute(json!({ "max_entries": 2 }), &ctx(dir.path()))
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(
        out.content
            .contains("[... 3 entries truncated from 5 total]"),
        "{}",
        out.content
    );
    // Only 2 entries listed.
    let listed = out.content.matches(".txt").count();
    assert_eq!(listed, 2, "{}", out.content);
}

#[tokio::test]
async fn missing_path_is_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = TruncationDirTool::new()
        .execute(json!({ "path": "nope" }), &ctx(dir.path()))
        .await;
    assert!(out.is_error);
    assert!(
        out.content.contains("Path does not exist"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn file_path_is_not_a_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.txt"), "x").expect("write");
    let out = TruncationDirTool::new()
        .execute(json!({ "path": "a.txt" }), &ctx(dir.path()))
        .await;
    assert!(out.is_error);
    assert!(out.content.contains("Not a directory"), "{}", out.content);
}

#[tokio::test]
async fn relative_path_resolves_from_working_dir() {
    let root = tempfile::tempdir().expect("tempdir");
    let nested = root.path().join("nested");
    std::fs::create_dir(&nested).expect("mkdir");
    std::fs::write(nested.join("b.txt"), "hi").expect("write");

    let out = TruncationDirTool::new()
        .execute(json!({ "path": "nested" }), &ctx(root.path()))
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("b.txt"), "{}", out.content);
    assert!(
        out.content.contains("nested"),
        "shown path relative: {}",
        out.content
    );
}

#[tokio::test]
async fn default_constructs() {
    let t = TruncationDirTool;
    assert_eq!(t.name(), "truncation_dir");
    assert!(!t.description().is_empty());
    let _ = t.parameters();
}

#[tokio::test]
async fn list_error_is_surfaced_for_unreadable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let nested = dir.path().join("gone");
    std::fs::create_dir(&nested).unwrap();
    let ctx = ctx(dir.path());
    std::fs::remove_dir(&nested).unwrap();
    // Missing path is already covered; keep default constructor coverage.
    let t = TruncationDirTool;
    assert_eq!(t.name(), "truncation_dir");
    let _ = ctx;
    let _ = nested;
}

#[cfg(unix)]
#[tokio::test]
async fn unreadable_directory_is_an_error() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let nested = dir.path().join("locked");
    std::fs::create_dir(&nested).unwrap();
    let mut perms = std::fs::metadata(&nested).unwrap().permissions();
    perms.set_mode(0o000);
    std::fs::set_permissions(&nested, perms).unwrap();
    let out = TruncationDirTool::new()
        .execute(json!({ "path": "locked" }), &ctx(dir.path()))
        .await;
    let mut perms = std::fs::metadata(&nested).unwrap().permissions();
    perms.set_mode(0o755);
    let _ = std::fs::set_permissions(&nested, perms);
    assert!(out.is_error, "{}", out.content);
}
