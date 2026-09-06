use super::*;
use crate::tool::ToolContext;
use whycodes_core::file_claims::FileClaimRegistry;

fn ctx(dir: &std::path::Path) -> ToolContext {
    ToolContext::new(dir.to_string_lossy().into_owned())
}

#[tokio::test]
async fn metadata_describes_write_tool() {
    let t = WriteTool::new();
    assert_eq!(t.name(), "write");
    assert!(t.description().contains("Write"));
    let params = t.parameters();
    assert_eq!(params["required"][0], "path");
    assert_eq!(params["required"][1], "content");
}

#[tokio::test]
async fn writes_relative_path_and_creates_parents() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = WriteTool::new()
        .execute(
            json!({ "path": "nested/a.txt", "content": "hello\nworld" }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("Wrote"), "{}", out.content);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("nested/a.txt")).expect("read"),
        "hello\nworld"
    );
}

#[tokio::test]
async fn writes_absolute_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let abs = dir.path().join("abs.txt");
    let out = WriteTool::new()
        .execute(
            json!({ "path": abs.to_string_lossy(), "content": "abs" }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(std::fs::read_to_string(&abs).expect("read"), "abs");
}

#[tokio::test]
async fn empty_content_writes_empty_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = WriteTool::new()
        .execute(json!({ "path": "empty.txt" }), &ctx(dir.path()))
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("(empty file)"), "{}", out.content);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("empty.txt")).expect("read"),
        ""
    );
}

#[tokio::test]
async fn file_claim_conflict_blocks_write() {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("claimed.txt");
    std::fs::write(&target, "old").expect("seed");

    let claims = FileClaimRegistry::new();
    assert!(matches!(
        claims.try_claim("other", "other-agent", &target),
        whycodes_core::file_claims::ClaimResult::Acquired
    ));

    let mut c = ctx(dir.path());
    c.file_claims = Some(claims);
    c.agent_id = Some("me".into());
    c.agent_label = Some("me-agent".into());

    let out = WriteTool::new()
        .execute(json!({ "path": "claimed.txt", "content": "new" }), &c)
        .await;
    assert!(out.is_error);
    assert!(out.content.contains("File conflict"), "{}", out.content);
    assert_eq!(std::fs::read_to_string(&target).expect("read"), "old");
}

#[tokio::test]
async fn default_constructs() {
    assert_eq!(WriteTool.name(), "write");
}

#[tokio::test]
async fn parent_dir_create_and_atomic_write_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let blocker = dir.path().join("blocked");
    std::fs::write(&blocker, "file-not-dir").unwrap();
    let nested = blocker.join("child.txt");
    let out = WriteTool::new()
        .execute(
            json!({ "path": nested.to_string_lossy(), "content": "x" }),
            &ctx(dir.path()),
        )
        .await;
    assert!(out.is_error, "{}", out.content);
    assert!(
        out.content.contains("Error creating directory"),
        "{}",
        out.content
    );

    let as_dir = dir.path().join("as-dir");
    std::fs::create_dir(&as_dir).unwrap();
    let out = WriteTool::new()
        .execute(
            json!({ "path": as_dir.to_string_lossy(), "content": "x" }),
            &ctx(dir.path()),
        )
        .await;
    assert!(out.is_error, "{}", out.content);
    assert!(
        out.content.contains("Error writing file"),
        "{}",
        out.content
    );
}

#[cfg(unix)]
#[tokio::test]
async fn write_atomic_fails_on_readonly_parent() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let parent = dir.path().join("rodir");
    std::fs::create_dir(&parent).unwrap();
    let mut perms = std::fs::metadata(&parent).unwrap().permissions();
    perms.set_mode(0o555);
    std::fs::set_permissions(&parent, perms).unwrap();
    let path = parent.join("ro.txt");
    let out = WriteTool::new()
        .execute(
            json!({ "path": path.to_string_lossy(), "content": "new" }),
            &ctx(dir.path()),
        )
        .await;
    let mut perms = std::fs::metadata(&parent).unwrap().permissions();
    perms.set_mode(0o755);
    let _ = std::fs::set_permissions(&parent, perms);
    assert!(out.is_error, "{}", out.content);
    assert!(
        out.content.contains("Error writing file"),
        "{}",
        out.content
    );
}
