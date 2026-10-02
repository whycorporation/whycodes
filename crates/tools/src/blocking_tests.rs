use super::*;

#[test]
fn blocking_module_loads() {
    assert!(!module_path!().is_empty());
}

#[tokio::test]
async fn tool_ok_and_join_failure() {
    let r = tool(Box::new(|| ToolResult {
        tool_call_id: String::new(),
        content: "ok".into(),
        is_error: false,
    }))
    .await;
    assert!(!r.is_error);
    assert_eq!(r.content, "ok");
    let echoed = tool_join_error("boom");
    assert!(echoed.is_error);
    assert!(echoed.content.contains("boom"), "{}", echoed.content);

    let handle = tokio::spawn(async {
        tool(Box::new(|| -> ToolResult {
            panic!("boom");
        }))
        .await
    });
    let failed = handle.await.expect("join outer");
    assert!(failed.is_error);
    assert!(
        failed.content.contains("background task failed"),
        "{}",
        failed.content
    );
}
