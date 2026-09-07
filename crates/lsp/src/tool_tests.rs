use super::*;
use crate::config::LspServerSpec;
use whycodes_core::tool::ToolContext;

#[test]
fn describes_itself() {
    let tool = LspTool::default();
    assert_eq!(tool.name(), "lsp");
    assert!(tool.description().contains("diagnostics"));
    let p = tool.parameters();
    assert_eq!(p["properties"]["action"]["enum"][0], "diagnostics");
    assert_eq!(p["properties"]["action"]["enum"][3], "references");
    assert!(p["required"].as_array().unwrap().contains(&json!("action")));
}

#[tokio::test]
async fn requires_file_path() {
    let tool = LspTool::new();
    let ctx = ToolContext::new("/tmp");
    let result = tool.execute(json!({ "action": "diagnostics" }), &ctx).await;
    assert!(result.is_error);
    assert!(result.content.contains("file_path"));
}

#[tokio::test]
async fn errors_when_the_extension_cannot_be_determined() {
    let tool = LspTool::new();
    let ctx = ToolContext::new("/tmp");
    let result = tool
        .execute(
            json!({ "action": "diagnostics", "file_path": "/tmp/README" }),
            &ctx,
        )
        .await;
    assert!(result.is_error);
    assert!(result.content.contains("file extension"));
}

#[tokio::test]
async fn reports_missing_language_server_for_unknown_extension() {
    let tool = LspTool::new();
    let ctx = ToolContext::new("/tmp");
    let result = tool
        .execute(
            json!({ "action": "diagnostics", "file_path": "/tmp/x.zzz" }),
            &ctx,
        )
        .await;
    assert!(result.is_error);
    assert!(
        result
            .content
            .contains("No language server configured for '.zzz'")
    );
}

async fn run(action: &str, path: &str) -> ToolResult {
    let tool = LspTool::new();
    let ctx = ToolContext::new("/tmp");
    tool.execute(
        json!({
            "action": action,
            "file_path": path,
            "line": 1,
            "character": 1
        }),
        &ctx,
    )
    .await
}

#[tokio::test]
async fn unknown_action_is_an_error() {
    let result = run("nope", "/tmp/x.whycodes_lsp_fake").await;
    assert!(result.is_error);
    assert!(result.content.contains("Unknown action"));
}

#[tokio::test]
async fn start_failure_is_reported() {
    let result = run("diagnostics", "/tmp/x.whycodes_lsp_missing").await;
    assert!(result.is_error);
    assert!(
        result.content.contains("Failed to start")
            || result.content.contains("was not found on PATH")
    );
}

#[tokio::test]
async fn open_document_failure_is_reported() {
    let tool = LspTool::new();
    let client = crate::client::start_test_client("ok", false).await.unwrap();
    client.kill_for_test().await;
    tool.insert_client("whycodes_lsp_failopen", Arc::new(client))
        .await;
    let ctx = ToolContext::new("/tmp");
    let result = tool
        .execute(
            json!({ "action": "hover", "file_path": "/tmp/x.whycodes_lsp_failopen" }),
            &ctx,
        )
        .await;
    assert!(result.is_error);
    assert!(result.content.contains("Error opening document"));
}

#[tokio::test]
async fn diagnostics_hover_definition_and_references() {
    let tool = LspTool::new();
    let ctx = ToolContext::new("/tmp");
    let path = "/tmp/x.whycodes_lsp_fake";
    let diags = tool
        .execute(json!({ "action": "diagnostics", "file_path": path }), &ctx)
        .await;
    assert!(!diags.is_error, "{}", diags.content);
    assert!(diags.content.contains("boom"));
    let cached = tool
        .execute(json!({ "action": "diagnostics", "file_path": path }), &ctx)
        .await;
    assert!(!cached.is_error);
    let hover = tool
        .execute(
            json!({ "action": "hover", "file_path": path, "line": 2, "character": 3 }),
            &ctx,
        )
        .await;
    assert_eq!(hover.content, "hello");
    let defn = tool
        .execute(
            json!({ "action": "definition", "file_path": path, "line": 1, "character": 1 }),
            &ctx,
        )
        .await;
    assert!(defn.content.contains("file:///tmp/a.rs"));
    let refs = tool
        .execute(
            json!({ "action": "references", "file_path": path, "line": 1, "character": 1 }),
            &ctx,
        )
        .await;
    assert!(refs.content.contains("file:///tmp/a.rs:1:1"));
}

#[tokio::test]
async fn empty_lsp_results_are_described() {
    let path = "/tmp/x.whycodes_lsp_empty";
    let diags = run("diagnostics", path).await;
    assert_eq!(diags.content, "No diagnostics found.");
    let hover = run("hover", path).await;
    assert!(hover.content.contains("No hover information"));
    let defn = run("definition", path).await;
    assert_eq!(defn.content, "No definition found.");
    let refs = run("references", path).await;
    assert_eq!(refs.content, "No references found.");
}

#[tokio::test]
async fn action_errors_are_reported() {
    let path = "/tmp/x.whycodes_lsp_err";
    for action in ["diagnostics", "hover", "definition", "references"] {
        let result = run(action, path).await;
        assert!(result.is_error, "{action}: {}", result.content);
        assert!(result.content.contains("Error"), "{action}");
    }
}

#[tokio::test]
async fn missing_line_defaults_to_zero() {
    let tool = LspTool::new();
    let ctx = ToolContext::new("/tmp");
    let result = tool
        .execute(
            json!({ "action": "hover", "file_path": "/tmp/x.whycodes_lsp_fake" }),
            &ctx,
        )
        .await;
    assert!(!result.is_error, "{}", result.content);
    assert_eq!(result.content, "hello");
}

#[tokio::test]
async fn opening_a_dead_client_is_an_error() {
    let tool = LspTool::new();
    let client = crate::client::start_test_client("ok", false).await.unwrap();
    client.kill_for_test().await;
    tool.insert_client("whycodes_lsp_fake", Arc::new(client))
        .await;
    let ctx = ToolContext::new("/tmp");
    let result = tool
        .execute(
            json!({ "action": "hover", "file_path": "/tmp/x.whycodes_lsp_fake" }),
            &ctx,
        )
        .await;
    assert!(result.is_error);
    assert!(result.content.contains("Error opening document"));
}

#[tokio::test]
async fn type_definition_implementation_and_symbols() {
    let tool = LspTool::new();
    let ctx = ToolContext::new("/tmp");
    let path = "/tmp/x.whycodes_lsp_fake";
    let ty = tool
        .execute(
            json!({ "action": "type_definition", "file_path": path, "line": 1, "character": 1 }),
            &ctx,
        )
        .await;
    assert!(!ty.is_error, "{}", ty.content);
    assert!(ty.content.contains("file:///tmp/a.rs"));
    let impls = tool
        .execute(
            json!({ "action": "implementation", "file_path": path, "line": 1, "character": 1 }),
            &ctx,
        )
        .await;
    assert!(!impls.is_error, "{}", impls.content);
    let symbols = tool
        .execute(json!({ "action": "symbols", "file_path": path }), &ctx)
        .await;
    assert!(!symbols.is_error, "{}", symbols.content);
    assert!(symbols.content.contains("main"));
}

#[tokio::test]
async fn workspace_symbols_without_file_uses_a_cached_client() {
    let tool = LspTool::new();
    let ctx = ToolContext::new("/tmp");
    let path = "/tmp/x.whycodes_lsp_fake";
    let primed = tool
        .execute(json!({ "action": "diagnostics", "file_path": path }), &ctx)
        .await;
    assert!(!primed.is_error, "{}", primed.content);
    let symbols = tool
        .execute(json!({ "action": "symbols", "query": "main" }), &ctx)
        .await;
    assert!(!symbols.is_error, "{}", symbols.content);
    assert!(symbols.content.contains("main"));
}

#[tokio::test]
async fn workspace_symbols_without_client_reports_missing_rust_server() {
    let dir = tempfile::tempdir().unwrap();
    let tool = LspTool::new();
    let ctx = ToolContext::new(dir.path().to_str().unwrap());
    let result = tool
        .execute(json!({ "action": "symbols", "query": "main" }), &ctx)
        .await;
    assert!(result.is_error, "{}", result.content);
    assert!(
        result.content.contains("Language server")
            || result.content.contains("needs a project marker")
            || result.content.contains("was not found on PATH"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn overlay_can_disable_a_builtin_and_set_idle_timeout() {
    let mut overlay = LspSettings {
        idle_timeout_ms: Some(1),
        ..LspSettings::default()
    };
    overlay.servers.insert(
        "whycodes-lsp-fake".into(),
        LspServerSpec {
            disabled: Some(true),
            ..LspServerSpec::default()
        },
    );
    let tool = LspTool::with_overlay(&overlay);
    let ctx = ToolContext::new("/tmp");
    let result = tool
        .execute(
            json!({ "action": "diagnostics", "file_path": "/tmp/x.whycodes_lsp_fake" }),
            &ctx,
        )
        .await;
    assert!(result.is_error);
    assert!(
        result
            .content
            .contains("No language server configured for '.whycodes_lsp_fake'")
    );
}

#[tokio::test]
async fn idle_timeout_evicts_cached_clients() {
    let overlay = LspSettings {
        idle_timeout_ms: Some(1),
        ..LspSettings::default()
    };
    let tool = LspTool::with_overlay(&overlay);
    let client = crate::client::start_test_client("ok", false).await.unwrap();
    tool.insert_client("whycodes_lsp_fake", Arc::new(client))
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let ctx = ToolContext::new("/tmp");
    let result = tool
        .execute(
            json!({ "action": "hover", "file_path": "/tmp/x.whycodes_lsp_fake", "line": 1 }),
            &ctx,
        )
        .await;
    assert!(!result.is_error, "{}", result.content);
}

#[tokio::test]
async fn missing_root_marker_message_names_the_server() {
    let dir = tempfile::tempdir().unwrap();
    let tool = LspTool::new();
    let ctx = ToolContext::new(dir.path().to_str().unwrap());
    let path = dir.path().join("x.rs");
    let result = tool
        .execute(
            json!({ "action": "diagnostics", "file_path": path.to_str().unwrap() }),
            &ctx,
        )
        .await;
    assert!(result.is_error);
    assert!(
        result.content.contains("rust-analyzer")
            && (result.content.contains("needs a project marker")
                || result.content.contains("was not found on PATH")),
        "{}",
        result.content
    );
}
