use super::*;
use crate::tool::ToolContext;

#[test]
fn code_mode_module_loads() {
    assert!(!module_path!().is_empty());
}

#[tokio::test]
async fn execute_reads_file_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn x() {}\n").unwrap();
    let t = CodeModeTool;
    assert_eq!(t.name(), "code_mode");
    assert!(!t.description().is_empty());
    let params = t.parameters();
    assert_eq!(
        params["required"],
        serde_json::json!(["path", "instruction"])
    );
    let ctx = ToolContext::new(dir.path().to_string_lossy());
    let ok = t
        .execute(
            serde_json::json!({
                "path": "a.rs",
                "instruction": "rename x",
                "language": "rust"
            }),
            &ctx,
        )
        .await;
    assert!(!ok.is_error, "{}", ok.content);
    assert!(ok.content.contains("rename x"), "{}", ok.content);
    assert!(ok.content.contains("Language: rust"), "{}", ok.content);
    assert!(ok.content.contains("fn x()"), "{}", ok.content);

    let abs = dir.path().join("a.rs");
    let via_abs = t
        .execute(
            serde_json::json!({
                "path": abs.to_string_lossy(),
                "instruction": "noop"
            }),
            &ctx,
        )
        .await;
    assert!(!via_abs.is_error, "{}", via_abs.content);

    let miss = t
        .execute(
            serde_json::json!({"path": "gone.rs", "instruction": "x"}),
            &ctx,
        )
        .await;
    assert!(miss.is_error, "{}", miss.content);
    assert!(miss.content.contains("Error reading"), "{}", miss.content);
}
