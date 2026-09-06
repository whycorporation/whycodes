use super::*;
use crate::tool::ToolContext;
use std::time::Duration;
use whycodes_index::IndexOptions;

fn ctx(dir: &std::path::Path) -> ToolContext {
    ToolContext::new(dir.to_string_lossy().into_owned())
}

#[tokio::test]
async fn metadata_describes_glob_tool() {
    let t = GlobTool::new();
    assert_eq!(t.name(), "glob");
    assert!(t.description().contains("glob"));
    let params = t.parameters();
    assert_eq!(params["required"][0], "pattern");
}

#[tokio::test]
async fn missing_pattern_is_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = GlobTool::new().execute(json!({}), &ctx(dir.path())).await;
    assert!(out.is_error);
    assert!(out.content.contains("Missing required"), "{}", out.content);
}

#[tokio::test]
async fn invalid_pattern_is_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = GlobTool::new()
        .execute(json!({ "pattern": "[" }), &ctx(dir.path()))
        .await;
    assert!(out.is_error);
    assert!(
        out.content.contains("Invalid glob pattern"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn missing_root_is_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = GlobTool::new()
        .execute(
            json!({ "pattern": "*.rs", "path": "nope" }),
            &ctx(dir.path()),
        )
        .await;
    assert!(out.is_error);
    assert!(out.content.contains("Path not found"), "{}", out.content);
}

#[tokio::test]
async fn finds_files_by_suffix_and_skips_heavy_dirs() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
    std::fs::create_dir_all(dir.path().join("target/debug")).expect("mkdir");
    std::fs::write(dir.path().join("src/main.rs"), "fn main(){}").expect("write");
    std::fs::write(dir.path().join("src/lib.rs"), "// lib").expect("write");
    std::fs::write(dir.path().join("target/debug/foo.rs"), "bin").expect("write");

    let out = GlobTool::new()
        .execute(json!({ "pattern": "*.rs" }), &ctx(dir.path()))
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("src/main.rs"), "{}", out.content);
    assert!(out.content.contains("src/lib.rs"), "{}", out.content);
    assert!(
        !out.content.contains("foo.rs"),
        "target/ pruned: {}",
        out.content
    );
    assert!(out.content.contains("Found 2 files"), "{}", out.content);
}

#[tokio::test]
async fn no_matches_is_not_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.txt"), "x").expect("write");
    let out = GlobTool::new()
        .execute(json!({ "pattern": "*.rs" }), &ctx(dir.path()))
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("No files matched"), "{}", out.content);
}

#[tokio::test]
async fn path_pattern_matches_relative_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
    std::fs::write(dir.path().join("src/main.rs"), "x").expect("write");
    std::fs::write(dir.path().join("other.rs"), "x").expect("write");

    let out = GlobTool::new()
        .execute(json!({ "pattern": "src/*.rs" }), &ctx(dir.path()))
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("src/main.rs"), "{}", out.content);
    assert!(!out.content.contains("other.rs"), "{}", out.content);
    assert!(out.content.contains("Found 1 file"), "{}", out.content);
}

#[tokio::test]
async fn max_results_caps_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    for i in 0..5 {
        std::fs::write(dir.path().join(format!("f{i}.txt")), "x").expect("write");
    }
    let out = GlobTool::new()
        .execute(
            json!({ "pattern": "*.txt", "max_results": 2 }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("showing 2 of"), "{}", out.content);
}

#[tokio::test]
async fn warm_index_serves_matches() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
    std::fs::write(dir.path().join("src/a.rs"), "x").expect("write");
    std::fs::write(dir.path().join("README.md"), "hi").expect("write");

    let idx = whycodes_index::WorkspaceIndex::start_with(
        vec![dir.path().to_path_buf()],
        IndexOptions {
            watch: false,
            threads: 1,
            ..Default::default()
        },
    );
    assert!(idx.wait_ready(Duration::from_secs(10)));
    let mut c = ctx(dir.path());
    c.file_index = Some(idx);

    let out = GlobTool::new()
        .execute(json!({ "pattern": "**/*.rs" }), &c)
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("src/a.rs"), "{}", out.content);
    assert!(!out.content.contains("README.md"), "{}", out.content);
}

#[tokio::test]
async fn hidden_file_pattern_walks_instead_of_index() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join(".env"), "SECRET=1").expect("write");
    std::fs::write(dir.path().join("visible.txt"), "x").expect("write");

    let idx = whycodes_index::WorkspaceIndex::start_with(
        vec![dir.path().to_path_buf()],
        IndexOptions {
            watch: false,
            threads: 1,
            ..Default::default()
        },
    );
    assert!(idx.wait_ready(Duration::from_secs(10)));
    let mut c = ctx(dir.path());
    c.file_index = Some(idx);

    let out = GlobTool::new()
        .execute(json!({ "pattern": ".env" }), &c)
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains(".env"), "{}", out.content);
}

#[tokio::test]
async fn remaining_glob_edges() {
    let t = GlobTool;
    assert_eq!(t.name(), "glob");
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "x").unwrap();
    let abs_pat = dir.path().join("*.rs").to_string_lossy().into_owned();
    let out = t
        .execute(
            serde_json::json!({"pattern": abs_pat, "path": dir.path().to_string_lossy()}),
            &ctx(dir.path()),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
}

#[tokio::test]
async fn fallback_glob_skips_heavy_dirs() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("target/debug")).unwrap();
    std::fs::write(dir.path().join("target/debug/foo.rs"), "x").unwrap();
    let abs_pat = dir
        .path()
        .join("target/**/*.rs")
        .to_string_lossy()
        .into_owned();
    let out = GlobTool::new()
        .execute(
            serde_json::json!({"pattern": abs_pat, "path": dir.path().to_string_lossy()}),
            &ctx(dir.path()),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
}
