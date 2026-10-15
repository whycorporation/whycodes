use super::*;

#[test]
fn after_turn_is_safe_to_call() {
    after_turn();
    after_turn();
}

#[test]
fn after_eviction_trims_only_when_something_left() {
    after_eviction(0);
    after_eviction(2);
}

#[tokio::test]
async fn watchdog_keeps_checking_until_aborted() {
    let task = spawn_retention_watchdog(Duration::from_millis(5), u64::MAX);
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!task.is_finished());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
}
