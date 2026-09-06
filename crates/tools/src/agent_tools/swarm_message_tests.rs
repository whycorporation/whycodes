use super::*;
use whycodes_core::SwarmHub;

#[tokio::test]
async fn send_reaches_inbox() {
    let hub = SwarmHub::new();
    let mut ctx = ToolContext::unsandboxed(".");
    ctx.swarm_hub = Some(hub.clone());
    ctx.agent_id = Some("worker-0".into());
    let result = SwarmMsgTool::new()
        .execute(
            json!({"to": "worker-1", "text": "file a.rs is yours"}),
            &ctx,
        )
        .await;
    assert!(!result.is_error, "{}", result.content);
    let got = hub.drain("worker-1");
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].from, "worker-0");
    assert_eq!(got[0].text, "file a.rs is yours");
}

#[tokio::test]
async fn missing_hub_and_args() {
    let t = SwarmMsgTool;
    assert_eq!(t.name(), "swarm_msg");
    assert!(!t.description().is_empty());
    assert_eq!(t.parameters()["required"], json!(["to", "text"]));
    let no_hub = t
        .execute(
            json!({"to": "all", "text": "hi"}),
            &ToolContext::unsandboxed("."),
        )
        .await;
    assert!(no_hub.is_error);
    assert!(
        no_hub.content.contains("only available"),
        "{}",
        no_hub.content
    );

    let mut ctx = ToolContext::unsandboxed(".");
    ctx.swarm_hub = Some(SwarmHub::new());
    ctx.agent_label = Some("parent".into());
    let missing = t.execute(json!({"to": " ", "text": ""}), &ctx).await;
    assert!(missing.is_error);
    let ok = t.execute(json!({"to": "all", "text": "hello"}), &ctx).await;
    assert!(!ok.is_error, "{}", ok.content);
}
