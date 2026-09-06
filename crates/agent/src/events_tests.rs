use super::*;
use std::time::Instant;

#[test]
fn turn_opts_new_defaults() {
    let opts = TurnOpts::new("p", "m", "k");
    assert_eq!(opts.provider_name, "p");
    assert_eq!(opts.model, "m");
    assert_eq!(opts.api_key, "k");
    assert!(opts.max_turns.is_none());
    assert!(opts.events.is_none());
    assert!(opts.cancel.is_none());
}

#[tokio::test]
async fn wait_until_cancelled_resolves_when_flag_set() {
    let flag = new_cancel_flag();
    let opt = Some(Arc::clone(&flag));
    let t0 = Instant::now();
    let waiter = tokio::spawn(async move {
        wait_until_cancelled(&opt).await;
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    request_cancel(&flag);
    waiter.await.expect("join");
    assert!(t0.elapsed() < Duration::from_secs(2));
}

#[test]
fn request_cancel_is_visible_to_is_cancelled() {
    let flag = new_cancel_flag();
    let opt = Some(Arc::clone(&flag));
    assert!(!is_cancelled(&opt));
    request_cancel(&flag);
    assert!(is_cancelled(&opt));
}

#[test]
fn is_cancelled_none_is_false() {
    assert!(!is_cancelled(&None));
}

#[tokio::test]
async fn wait_until_cancelled_none_races_with_timeout() {
    let waiter = wait_until_cancelled(&None);
    let raced = tokio::time::timeout(Duration::from_millis(30), waiter).await;
    assert!(raced.is_err(), "None flag must stay pending");
}

#[test]
fn emit_none_is_noop_and_some_delivers() {
    emit(&None, TurnEvent::Status("nope".into()));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    emit(&Some(tx), TurnEvent::Status("hi".into()));
    let Ok(TurnEvent::Status(s)) = rx.try_recv() else {
        panic!("expected status");
    };
    assert_eq!(s, "hi");
}
