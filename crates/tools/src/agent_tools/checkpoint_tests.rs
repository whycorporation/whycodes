use super::*;

#[tokio::test]
async fn checkpoint_rejects_empty_goal() {
    let t = CheckpointTool::new();
    let r = t
        .execute(json!({"goal": "  "}), &ToolContext::new("/tmp"))
        .await;
    assert!(r.is_error);
}

#[tokio::test]
async fn checkpoint_echoes_goal() {
    let t = CheckpointTool::new();
    let r = t
        .execute(json!({"goal": "find the leak"}), &ToolContext::new("/tmp"))
        .await;
    assert!(!r.is_error);
    assert!(r.content.contains("find the leak"));
    assert_eq!(t.name(), "checkpoint");
    assert!(!t.description().is_empty());
}

#[tokio::test]
async fn rewind_rejects_empty_report() {
    let t = RewindTool::new();
    let r = t
        .execute(json!({"report": ""}), &ToolContext::new("/tmp"))
        .await;
    assert!(r.is_error);
    assert_eq!(t.name(), "rewind");
}

#[tokio::test]
async fn rewind_echoes_report() {
    let t = RewindTool::new();
    let r = t
        .execute(
            json!({"report": "root cause is X"}),
            &ToolContext::new("/tmp"),
        )
        .await;
    assert!(!r.is_error);
    assert!(r.content.contains("root cause is X"));
}

#[tokio::test]
async fn defaults_and_parameters() {
    let c = CheckpointTool::default();
    assert_eq!(c.name(), "checkpoint");
    let _ = c.parameters();
    let r = RewindTool::default();
    assert_eq!(r.name(), "rewind");
    assert!(!r.description().is_empty());
    let _ = r.parameters();
}
