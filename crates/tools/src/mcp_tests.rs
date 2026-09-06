use super::*;
use crate::tool::ToolContext;
use serde_json::json;
use std::sync::Mutex;

/// Records the args it was called with so tests can assert forwarding.
struct FakeCaller {
    calls: Mutex<Vec<serde_json::Value>>,
    reply: Result<String, String>,
}
impl McpCaller for FakeCaller {
    fn call_mcp_tool<'a>(
        &'a self,
        _tool_name: &'a str,
        arguments: serde_json::Value,
    ) -> futures::future::BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(arguments);
            self.reply.clone()
        })
    }
}

fn bridge(reply: Result<String, String>) -> (McpToolBridge, Arc<FakeCaller>) {
    let caller = Arc::new(FakeCaller {
        calls: Mutex::new(Vec::new()),
        reply,
    });
    let b = McpToolBridge::new(
        caller.clone(),
        "server_tool".into(),
        "Does a thing".into(),
        json!({ "type": "object", "properties": { "x": { "type": "string" } } }),
    );
    (b, caller)
}

#[tokio::test]
async fn metadata_passes_through_name_description_schema() {
    let (b, _) = bridge(Ok("ok".into()));
    assert_eq!(b.name(), "server_tool");
    assert_eq!(b.description(), "Does a thing");
    assert_eq!(b.parameters()["properties"]["x"]["type"], "string");
}

#[tokio::test]
async fn execute_forwards_args_and_returns_text() {
    let (b, caller) = bridge(Ok("done".into()));
    let out = b
        .execute(json!({ "x": "v" }), &ToolContext::new("/tmp"))
        .await;
    assert!(!out.is_error);
    assert_eq!(out.content, "done");
    assert_eq!(caller.calls.lock().unwrap().len(), 1);
    assert_eq!(caller.calls.lock().unwrap()[0]["x"], "v");
}

#[tokio::test]
async fn execute_surfaces_caller_error() {
    let (b, _) = bridge(Err("boom".into()));
    let out = b.execute(json!({}), &ToolContext::new("/tmp")).await;
    assert!(out.is_error);
    assert_eq!(out.content, "MCP tool 'server_tool' error: boom");
}
