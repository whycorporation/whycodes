use super::*;
use crate::tool::ToolContext;

fn ctx(dir: &std::path::Path) -> ToolContext {
    ToolContext::new(dir.to_string_lossy().into_owned())
}

fn allow(dir: &std::path::Path, lines: &str) {
    let why = dir.join(".whycodes");
    std::fs::create_dir_all(&why).expect("mkdir .whycodes");
    std::fs::write(why.join("external_dirs_allowed"), lines).expect("write allowlist");
}

#[tokio::test]
async fn metadata_describes_external_directory_tool() {
    let t = ExternalDirectoryTool::new();
    assert_eq!(t.name(), "external_directory");
    assert!(t.description().contains("outside"));
    let params = t.parameters();
    assert_eq!(params["required"][0], "path");
    assert_eq!(params["properties"]["action"]["enum"][0], "read");
}

#[tokio::test]
async fn denies_when_allowlist_is_missing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside");
    std::fs::write(outside.path().join("secret.txt"), "nope").expect("write");

    let out = ExternalDirectoryTool::new()
        .execute(
            json!({ "path": outside.path().join("secret.txt").to_string_lossy(), "action": "read" }),
            &ctx(dir.path()),
        )
        .await;
    assert!(out.is_error);
    assert!(out.content.contains("Access denied"), "{}", out.content);
}

#[tokio::test]
async fn read_and_list_when_path_is_allowed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside");
    std::fs::write(outside.path().join("note.txt"), "hello").expect("write");
    std::fs::create_dir(outside.path().join("sub")).expect("mkdir");
    allow(
        dir.path(),
        &format!("# comment\n\n{}\n", outside.path().display()),
    );

    let read = ExternalDirectoryTool::new()
        .execute(
            json!({
                "path": outside.path().join("note.txt").to_string_lossy(),
                "action": "read"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!read.is_error, "{}", read.content);
    assert_eq!(read.content, "hello");

    let list = ExternalDirectoryTool::new()
        .execute(
            json!({
                "path": outside.path().to_string_lossy(),
                "action": "list"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!list.is_error, "{}", list.content);
    assert!(list.content.contains("note.txt"), "{}", list.content);
    assert!(list.content.contains("sub"), "{}", list.content);
    assert!(list.content.contains("d"), "{}", list.content);
}

#[tokio::test]
async fn unknown_action_is_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside");
    allow(dir.path(), &format!("{}\n", outside.path().display()));

    let out = ExternalDirectoryTool::new()
        .execute(
            json!({
                "path": outside.path().to_string_lossy(),
                "action": "wipe"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(out.is_error);
    assert!(out.content.contains("Unknown action"), "{}", out.content);
}

#[tokio::test]
async fn list_on_a_file_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside");
    std::fs::write(outside.path().join("f.txt"), "x").expect("write");
    allow(dir.path(), &format!("{}\n", outside.path().display()));

    let out = ExternalDirectoryTool::new()
        .execute(
            json!({
                "path": outside.path().join("f.txt").to_string_lossy(),
                "action": "list"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(out.is_error);
    assert!(
        out.content.contains("Error listing directory"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn read_on_a_directory_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside");
    allow(dir.path(), &format!("{}\n", outside.path().display()));

    // Missing paths fail canonicalize and are denied; a directory is
    // allowed but cannot be read as text.
    let out = ExternalDirectoryTool::new()
        .execute(
            json!({
                "path": outside.path().to_string_lossy(),
                "action": "read"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(out.is_error);
    assert!(
        out.content.contains("Error reading file"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn relative_path_resolves_from_working_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    let nested = dir.path().join("ext");
    std::fs::create_dir(&nested).expect("mkdir");
    std::fs::write(nested.join("a.txt"), "rel").expect("write");
    allow(dir.path(), &format!("{}\n", nested.display()));

    let out = ExternalDirectoryTool::new()
        .execute(
            json!({ "path": "ext/a.txt", "action": "read" }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(out.content, "rel");
}

#[tokio::test]
async fn empty_directory_list_is_labelled() {
    let dir = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside");
    allow(dir.path(), &format!("{}\n", outside.path().display()));

    let out = ExternalDirectoryTool::new()
        .execute(
            json!({
                "path": outside.path().to_string_lossy(),
                "action": "list"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("is empty"), "{}", out.content);
}

#[tokio::test]
async fn default_action_is_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside");
    std::fs::write(outside.path().join("a.txt"), "def").expect("write");
    allow(dir.path(), &format!("{}\n", outside.path().display()));

    let out = ExternalDirectoryTool::new()
        .execute(
            json!({ "path": outside.path().join("a.txt").to_string_lossy() }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(out.content, "def");
}

#[tokio::test]
async fn default_constructs() {
    assert_eq!(
        ExternalDirectoryTool::default().name(),
        "external_directory"
    );
    skip_dir_entry("boom", "entry metadata failed; skipping");
    skip_metadata_failed("boom");
    skip_entry_failed("boom");
    assert!(format_dir_entry(Err(std::io::Error::other("gone"))).is_none());
    assert!(format_dir_meta(Err(std::io::Error::other("gone")), "x").is_none());
    assert_eq!(entry_type_from_flags(true, false), "d");
    assert_eq!(entry_type_from_flags(false, true), "l");
    assert_eq!(entry_type_from_flags(false, false), "-");
    let dir = tempfile::tempdir().expect("tempdir");
    let meta = std::fs::metadata(dir.path()).unwrap();
    assert_eq!(entry_type_label(&meta), "d");
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "x").unwrap();
    let file_meta = std::fs::metadata(&file).unwrap();
    assert_eq!(entry_type_label(&file_meta), "-");
}

#[tokio::test]
async fn deny_missing_path_and_broken_allowlist_entry() {
    let dir = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside");
    allow(
        dir.path(),
        &format!("/nonexistent-xyz-allow\n{}\n", outside.path().display()),
    );
    let missing = ExternalDirectoryTool::new()
        .execute(
            json!({
                "path": outside.path().join("gone.txt").to_string_lossy(),
                "action": "read"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(missing.is_error, "{}", missing.content);
    assert!(
        missing.content.contains("Access denied"),
        "{}",
        missing.content
    );

    let other = tempfile::tempdir().expect("other");
    std::fs::write(other.path().join("x.txt"), "nope").unwrap();
    let denied = ExternalDirectoryTool::new()
        .execute(
            json!({
                "path": other.path().join("x.txt").to_string_lossy(),
                "action": "read"
            }),
            &ctx(dir.path()),
        )
        .await;
    assert!(denied.is_error, "{}", denied.content);
}
