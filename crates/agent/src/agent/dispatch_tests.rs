use super::*;
use crate::events::TurnEvent;

#[test]
fn send_or_debug_delivers_then_logs_when_closed() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    send_or_debug(
        &tx,
        TurnEvent::Status("live".into()),
        "should not drop while the receiver is open",
    );
    match rx.try_recv() {
        Ok(TurnEvent::Status(s)) => assert_eq!(s, "live"),
        other => panic!("expected status, got {other:?}"),
    }
    drop(rx);
    send_or_debug(
        &tx,
        TurnEvent::Status("gone".into()),
        "swarm stale event dropped",
    );
    send_or_debug(
        &tx,
        TurnEvent::SwarmMessage {
            from: "a".into(),
            to: "b".into(),
            text: "hi".into(),
        },
        "swarm message event dropped",
    );
}

#[test]
fn append_cleanup_warning_keeps_ok_and_appends_err() {
    assert_eq!(append_cleanup_warning("body".into(), Ok(())), "body");
    let warned = append_cleanup_warning("body".into(), Err("path remains".into()));
    assert!(warned.contains("body"), "{warned}");
    assert!(warned.contains("Worktree cleanup warning"), "{warned}");
    assert!(warned.contains("path remains"), "{warned}");
}
