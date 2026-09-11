use super::*;
use crate::tool::ToolContext;

fn ctx() -> ToolContext {
    ToolContext::new("/tmp")
}

#[tokio::test]
async fn metadata_describes_truncate_tool() {
    let t = TruncateTool::new();
    assert_eq!(t.name(), "truncate");
    assert!(t.description().contains("Truncate"));
    let params = t.parameters();
    assert_eq!(params["required"][0], "text");
}

#[tokio::test]
async fn short_text_passes_through() {
    let out = TruncateTool::new()
        .execute(json!({ "text": "hello" }), &ctx())
        .await;
    assert!(!out.is_error);
    assert_eq!(out.content, "hello");
}

#[tokio::test]
async fn max_lines_caps_output() {
    let text = (0..10)
        .map(|i| format!("L{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let out = TruncateTool::new()
        .execute(
            json!({ "text": text, "max_lines": 3, "max_chars": 8000 }),
            &ctx(),
        )
        .await;
    assert!(!out.is_error);
    assert!(out.content.contains("L0"), "{}", out.content);
    assert!(
        !out.content.contains("L9"),
        "later lines dropped: {}",
        out.content
    );
}

#[tokio::test]
async fn missing_text_yields_empty() {
    let out = TruncateTool::new().execute(json!({}), &ctx()).await;
    assert!(!out.is_error);
    assert_eq!(out.content, "");
}

#[tokio::test]
async fn default_constructs() {
    assert_eq!(TruncateTool::default().name(), "truncate");
}
