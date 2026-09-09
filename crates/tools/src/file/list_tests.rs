use super::*;
use crate::tool::ToolContext;
use std::path::Path;
use std::time::Duration;
use whycodes_index::IndexOptions;

fn ctx(dir: &std::path::Path) -> ToolContext {
    ToolContext::new(dir.to_string_lossy().into_owned())
}

fn ctx_with_index(dir: &std::path::Path) -> ToolContext {
    let idx = whycodes_index::WorkspaceIndex::start_with(
        vec![dir.to_path_buf()],
        IndexOptions {
            watch: false,
            threads: 1,
            ..Default::default()
        },
    );
    assert!(
        idx.wait_ready(Duration::from_secs(10)),
        "index never became ready"
    );
    let mut c = ToolContext::new(dir.to_string_lossy().into_owned());
    c.file_index = Some(idx);
    c
}

#[tokio::test]
async fn metadata_describes_list_tool() {
    let t = ListTool::new();
    assert_eq!(t.name(), "list");
    assert!(t.description().contains("List files"));
    let params = t.parameters();
    assert_eq!(params["required"], json!([]));
    assert!(params["properties"].get("path").is_some());
    assert!(params["properties"].get("recursive").is_some());
}

#[tokio::test]
async fn lists_files_and_dirs_with_sizes() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.txt"), "hello").expect("write");
    std::fs::create_dir(dir.path().join("sub")).expect("mkdir");

    let out = ListTool::new().execute(json!({}), &ctx(dir.path())).await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("Contents of"), "{}", out.content);
    assert!(out.content.contains("a.txt"), "{}", out.content);
    assert!(
        out.content.contains("sub") && out.content.contains('/'),
        "dir marked with slash: {}",
        out.content
    );
    assert!(
        out.content.contains("5 B"),
        "5-byte file shown: {}",
        out.content
    );
    assert!(
        out.content.contains("1 directories, 1 files"),
        "{}",
        out.content
    );
    assert!(
        !out.content.contains("[recursive"),
        "non-recursive has no depth tag: {}",
        out.content
    );
}

#[tokio::test]
async fn empty_directory_is_labelled() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = ListTool::new().execute(json!({}), &ctx(dir.path())).await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("(empty)"), "{}", out.content);
    assert!(
        out.content.contains("0 directories, 0 files"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn missing_path_is_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = ListTool::new()
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
    let out = ListTool::new()
        .execute(json!({ "path": "a.txt" }), &ctx(dir.path()))
        .await;
    assert!(out.is_error);
    assert!(out.content.contains("Not a directory"), "{}", out.content);
    assert!(out.content.contains("use `read`"), "{}", out.content);
}

#[tokio::test]
async fn ignore_globs_drop_matching_names() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("keep.rs"), "x").expect("write");
    std::fs::write(dir.path().join("skip.tmp"), "x").expect("write");

    let out = ListTool::new()
        .execute(json!({ "ignore": ["*.tmp"] }), &ctx(dir.path()))
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("keep.rs"), "{}", out.content);
    assert!(!out.content.contains("skip.tmp"), "{}", out.content);
    assert!(
        out.content.contains("0 directories, 1 files"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn max_entries_truncates_and_notes_limit() {
    let dir = tempfile::tempdir().expect("tempdir");
    for i in 0..5 {
        std::fs::write(dir.path().join(format!("f{i}.txt")), "x").expect("write");
    }
    let out = ListTool::new()
        .execute(json!({ "max_entries": 2 }), &ctx(dir.path()))
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(
        out.content
            .contains("(showing first 2 — raise max_entries or narrow path)"),
        "{}",
        out.content
    );
    assert!(
        out.content.contains("0 directories, 5 files"),
        "totals count everything: {}",
        out.content
    );
    let listed = out.content.matches(".txt").count();
    assert_eq!(listed, 2, "{}", out.content);
}

#[tokio::test]
async fn recursive_walk_lists_nested_and_skips_heavy_dirs() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
    std::fs::create_dir_all(dir.path().join("target/debug")).expect("mkdir");
    std::fs::write(dir.path().join("src/main.rs"), "fn main(){}").expect("write");
    std::fs::write(dir.path().join("target/debug/foo.o"), "bin").expect("write");

    let out = ListTool::new()
        .execute(
            json!({ "recursive": true, "max_depth": 5 }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("src"), "{}", out.content);
    assert!(out.content.contains("src/main.rs"), "{}", out.content);
    // Heavy dir itself may appear, but its children are pruned.
    assert!(
        !out.content.contains("foo.o"),
        "target/ contents pruned: {}",
        out.content
    );
    assert!(
        out.content.contains("[recursive depth≤5]"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn recursive_max_depth_stops_descent() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("a/b")).expect("mkdir");
    std::fs::write(dir.path().join("a/b/c.txt"), "x").expect("write");

    let shallow = ListTool::new()
        .execute(
            json!({ "recursive": true, "max_depth": 1 }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!shallow.is_error, "{}", shallow.content);
    assert!(shallow.content.contains('a'), "{}", shallow.content);
    assert!(
        !shallow.content.contains("c.txt"),
        "depth 1 must not reach a/b/c.txt: {}",
        shallow.content
    );

    let deep = ListTool::new()
        .execute(
            json!({ "recursive": true, "max_depth": 3 }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!deep.is_error, "{}", deep.content);
    assert!(deep.content.contains("a/b/c.txt"), "{}", deep.content);
}

#[tokio::test]
async fn recursive_max_entries_truncates() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join("d")).expect("mkdir");
    for i in 0..4 {
        std::fs::write(dir.path().join(format!("d/f{i}.txt")), "x").expect("write");
    }
    let out = ListTool::new()
        .execute(
            json!({ "recursive": true, "max_depth": 5, "max_entries": 2 }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("showing first 2"), "{}", out.content);
    assert!(
        out.content.contains("[recursive depth≤5]"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn recursive_ignore_globs_apply_per_level() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join("src")).expect("mkdir");
    std::fs::write(dir.path().join("src/keep.rs"), "x").expect("write");
    std::fs::write(dir.path().join("src/skip.tmp"), "x").expect("write");

    let out = ListTool::new()
        .execute(
            json!({ "recursive": true, "ignore": ["*.tmp"], "max_depth": 3 }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("keep.rs"), "{}", out.content);
    assert!(!out.content.contains("skip.tmp"), "{}", out.content);
}

#[tokio::test]
async fn relative_path_resolves_from_working_dir() {
    let root = tempfile::tempdir().expect("tempdir");
    let nested = root.path().join("nested");
    std::fs::create_dir(&nested).expect("mkdir");
    std::fs::write(nested.join("b.txt"), "hi").expect("write");

    let out = ListTool::new()
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
async fn clamps_max_depth_and_max_entries() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.txt"), "x").expect("write");

    // 0 clamps to 1 — still lists the single file.
    let out = ListTool::new()
        .execute(json!({ "max_entries": 0 }), &ctx(dir.path()))
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("a.txt"), "{}", out.content);

    // Huge max_depth is clamped (20) and still tagged as recursive.
    let out = ListTool::new()
        .execute(
            json!({ "recursive": true, "max_depth": 99 }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(
        out.content.contains("[recursive depth≤20]"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn recursive_uses_warm_index_when_available() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
    std::fs::write(dir.path().join("src/lib.rs"), "// lib").expect("write");
    std::fs::write(dir.path().join("README.md"), "hi").expect("write");

    let out = ListTool::new()
        .execute(
            json!({ "recursive": true, "max_depth": 3 }),
            &ctx_with_index(dir.path()),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("README.md"), "{}", out.content);
    assert!(out.content.contains("src/lib.rs"), "{}", out.content);
    assert!(
        out.content.contains("[recursive depth≤3]"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn index_path_honours_ignore_depth_and_cap() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("a/b")).expect("mkdir");
    std::fs::write(dir.path().join("keep.rs"), "x").expect("write");
    std::fs::write(dir.path().join("skip.tmp"), "x").expect("write");
    std::fs::write(dir.path().join("a/b/deep.rs"), "x").expect("write");

    let ctx = ctx_with_index(dir.path());

    let ignored = ListTool::new()
        .execute(
            json!({ "recursive": true, "ignore": ["*.tmp"], "max_depth": 5 }),
            &ctx,
        )
        .await;
    assert!(!ignored.is_error, "{}", ignored.content);
    assert!(ignored.content.contains("keep.rs"), "{}", ignored.content);
    assert!(!ignored.content.contains("skip.tmp"), "{}", ignored.content);

    let shallow = ListTool::new()
        .execute(json!({ "recursive": true, "max_depth": 1 }), &ctx)
        .await;
    assert!(!shallow.is_error, "{}", shallow.content);
    assert!(
        !shallow.content.contains("deep.rs"),
        "depth 1 must skip a/b/deep.rs: {}",
        shallow.content
    );

    let capped = ListTool::new()
        .execute(
            json!({ "recursive": true, "max_depth": 5, "max_entries": 1 }),
            &ctx,
        )
        .await;
    assert!(!capped.is_error, "{}", capped.content);
    assert!(
        capped.content.contains("showing first 1"),
        "{}",
        capped.content
    );
}

#[tokio::test]
async fn index_outside_scope_falls_back_to_walk() {
    let indexed = tempfile::tempdir().expect("tempdir");
    let listed = tempfile::tempdir().expect("tempdir");
    std::fs::write(listed.path().join("only-walk.txt"), "x").expect("write");

    let mut c = ctx_with_index(indexed.path());
    c.working_dir = listed.path().to_string_lossy().into_owned();

    let out = ListTool::new()
        .execute(json!({ "recursive": true }), &c)
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(
        out.content.contains("only-walk.txt"),
        "walk fallback: {}",
        out.content
    );
}

#[tokio::test]
async fn malformed_ignore_entries_are_skipped() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.txt"), "x").expect("write");
    let out = ListTool::new()
        .execute(json!({ "ignore": [1, true, "a.txt"] }), &ctx(dir.path()))
        .await;
    assert!(!out.is_error, "{}", out.content);
    // Only the string glob is honoured; a.txt is ignored.
    assert!(!out.content.contains("a.txt"), "{}", out.content);
    assert!(out.content.contains("(empty)"), "{}", out.content);
}

#[tokio::test]
async fn default_constructs() {
    assert_eq!(ListTool::default().name(), "list");
    let listed = list_entries_error("cannot list".into());
    assert!(listed.is_error);
    assert_eq!(listed.content, "cannot list");
    let missing = listed_entries(Path::new("/nonexistent-xyz"), &[], 10);
    assert!(missing.is_err());
    let failed = listed_entries_failed(list_entries_error("cannot list".into()));
    assert!(failed.is_error);
    assert!(take_listed(Err(list_entries_error("cannot list".into()))).is_err());
    assert!(take_listed(listed_entries(Path::new("/nonexistent-xyz"), &[], 10)).is_err());
    let from_err = listing_from(Err(list_entries_error("cannot list".into())), ".", false, 1);
    assert!(from_err.is_error);
    let from_ok = listing_from(Ok((Vec::new(), false, 0, 0)), ".", false, 1);
    assert!(!from_ok.is_error);
    assert!(from_ok.content.contains("(empty)"));
}

#[tokio::test]
async fn list_dir_entries_error_is_surfaced() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("not-a-dir");
    std::fs::write(&file, "x").expect("write");
    // Point working dir at a real dir, but race the target into a file via absolute path.
    let out = ListTool::new()
        .execute(json!({ "path": file.to_string_lossy() }), &ctx(dir.path()))
        .await;
    assert!(out.is_error, "{}", out.content);
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
    let out = ListTool::new()
        .execute(json!({ "path": "locked" }), &ctx(dir.path()))
        .await;
    let mut perms = std::fs::metadata(&nested).unwrap().permissions();
    perms.set_mode(0o755);
    let _ = std::fs::set_permissions(&nested, perms);
    assert!(out.is_error, "{}", out.content);
}
