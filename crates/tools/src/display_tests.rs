use super::*;
use crate::tool::ToolContext;

fn ctx() -> ToolContext {
    ToolContext::new("/tmp")
}

#[tokio::test]
async fn metadata_describes_display_tool() {
    let t = DisplayTool::new();
    assert_eq!(t.name(), "display");
    assert!(t.description().contains("syntax highlighting"));
    let params = t.parameters();
    assert_eq!(params["required"][0], "content");
    assert_eq!(params["properties"]["format"]["enum"][0], "code");
}

#[tokio::test]
async fn code_format_highlights_when_language_given() {
    let t = DisplayTool::new();
    let out = t
        .execute(
            json!({ "content": "fn main() {}", "language": "rust", "format": "code" }),
            &ctx(),
        )
        .await;
    assert!(!out.is_error);
    // Highlighted output carries ANSI escape sequences.
    assert!(out.content.contains('\x1b'), "{}", out.content);
}

#[tokio::test]
async fn code_format_passes_plain_text_through_without_language() {
    let t = DisplayTool::new();
    let out = t
        .execute(
            json!({ "content": "hello world", "format": "code" }),
            &ctx(),
        )
        .await;
    assert!(!out.is_error);
    assert_eq!(out.content, "hello world");
}

#[tokio::test]
async fn code_format_detects_language_from_path_like_content() {
    let t = DisplayTool::new();
    // First line looks like a file path → language detected → ANSI codes.
    let out = t
        .execute(
            json!({ "content": "main.rs\nfn main() {}", "format": "code" }),
            &ctx(),
        )
        .await;
    assert!(!out.is_error);
    assert!(out.content.contains('\x1b'), "{}", out.content);
}

#[tokio::test]
async fn diff_format_colors_hunk_lines() {
    let t = DisplayTool::new();
    let out = t
        .execute(
            json!({ "content": "@@ -1,2 +1,2 @@\n-old\n+new", "format": "diff" }),
            &ctx(),
        )
        .await;
    assert!(!out.is_error);
    assert!(
        out.content.contains('\x1b'),
        "diff colored: {}",
        out.content
    );
}

#[tokio::test]
async fn table_and_unknown_formats_pass_through() {
    let t = DisplayTool::new();
    let out = t
        .execute(json!({ "content": "a,b\nc,d", "format": "table" }), &ctx())
        .await;
    assert_eq!(out.content, "a,b\nc,d");

    let out = t
        .execute(json!({ "content": "raw", "format": "bogus" }), &ctx())
        .await;
    assert_eq!(out.content, "raw");
}

#[tokio::test]
async fn missing_content_yields_empty_output() {
    let t = DisplayTool::new();
    let out = t.execute(json!({}), &ctx()).await;
    assert!(!out.is_error);
    assert_eq!(out.content, "");
}

#[tokio::test]
async fn default_constructs() {
    assert_eq!(DisplayTool.name(), "display");
}

#[tokio::test]
async fn text_format_passes_through() {
    let out = DisplayTool
        .execute(json!({ "content": "plain", "format": "text" }), &ctx())
        .await;
    assert!(!out.is_error);
    assert_eq!(out.content, "plain");
}

#[test]
fn format_display_content_covers_code_diff_and_passthrough() {
    let rust = format_display_content("fn main() {}", "rust", "code");
    assert!(rust.contains('\x1b'), "{rust}");
    assert_eq!(format_display_content("hello", "", "code"), "hello");
    let detected = format_display_content("main.rs\nfn main() {}", "", "code");
    assert!(detected.contains('\x1b'), "{detected}");
    let diff = format_display_content("@@ -1 +1 @@\n-old\n+new", "", "diff");
    assert!(diff.contains('\x1b'), "{diff}");
    assert_eq!(format_display_content("a,b", "", "table"), "a,b");
    assert_eq!(format_display_content("raw", "", "text"), "raw");
}
