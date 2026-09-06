use super::*;

#[test]
fn blocking_module_loads() {
    assert!(!module_path!().is_empty());
}

#[tokio::test]
async fn run_and_tool_ok() {
    let n = run(|| 7).await.unwrap();
    assert_eq!(n, 7);
    let r = tool(|| ToolResult {
        tool_call_id: String::new(),
        content: "ok".into(),
        is_error: false,
    })
    .await;
    assert!(!r.is_error);
    assert_eq!(r.content, "ok");
}

#[tokio::test]
async fn tool_maps_join_failure() {
    let handle = tokio::spawn(async {
        tool(|| {
            panic!("boom");
        })
        .await
    });
    let r = handle.await.expect("join outer");
    assert!(r.is_error);
    assert!(
        r.content.contains("background task failed"),
        "{}",
        r.content
    );
}
