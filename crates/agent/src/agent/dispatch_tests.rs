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

fn poison_mutex<T>(value: T) -> std::sync::Mutex<T> {
    let m = std::sync::Mutex::new(value);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _g = m.lock().unwrap();
        panic!("poison dispatch mutex");
    }));
    m
}

#[test]
fn dispatch_helpers_cover_poison_and_optional_sinks() {
    assert_eq!(optional_exit_suffix(Some(7)), " exit=7");
    assert_eq!(optional_exit_suffix(None), "");

    let cwd = poison_mutex(Some(std::path::PathBuf::from("/tmp/wt/w0")));
    clear_cwd_if_under(&cwd, std::path::Path::new("/tmp/wt/w0"));
    assert!(super::super::recover_lock(&cwd).is_none());

    let cwd = std::sync::Mutex::new(Some(std::path::PathBuf::from("/tmp/other")));
    clear_cwd_if_under(&cwd, std::path::Path::new("/tmp/wt/w0"));
    assert_eq!(
        super::super::recover_lock(&cwd).as_deref(),
        Some(std::path::Path::new("/tmp/other"))
    );

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    assert!(first_event_sink(Some(&tx), None).is_some());
    assert!(first_event_sink(None, Some(&tx)).is_some());
    assert!(first_event_sink(None, None).is_none());

    let pending = poison_mutex(whycodes_core::types::Usage::default());
    fold_pending_usage(&pending, &whycodes_core::types::Usage::default());
    let usage = whycodes_core::types::Usage {
        input_tokens: 3,
        ..Default::default()
    };
    fold_pending_usage(&pending, &usage);
    assert_eq!(super::super::recover_lock(&pending).input_tokens, 3);
}
