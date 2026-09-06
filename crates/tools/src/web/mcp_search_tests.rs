use super::*;
use crate::tool::ToolContext;

#[tokio::test]
async fn metadata_and_source_marker() {
    let t = McpWebSearchTool::new();
    assert_eq!(t.name(), "mcp_websearch");
    assert_eq!(t.source(), "mcp");
    assert!(t.description().contains("MCP"));
    let params = t.parameters();
    assert_eq!(params["required"][0], "query");
}

#[tokio::test]
async fn execute_forwards_empty_query_error() {
    let out = McpWebSearchTool::new()
        .execute(json!({}), &ToolContext::new("/tmp"))
        .await;
    assert!(out.is_error);
    assert!(out.content.contains("Query is required"), "{}", out.content);
}

#[test]
fn default_constructs() {
    assert_eq!(McpWebSearchTool::default().name(), "mcp_websearch");
}
