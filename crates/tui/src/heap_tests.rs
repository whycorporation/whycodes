use super::*;

#[test]
fn release_retained_heap_is_safe_to_call() {
    release_retained_heap("unit_test");
}

#[test]
fn debounce_skips_within_the_interval() {
    release_retained_heap("debounce_setup");
    assert!(!release_retained_heap_debounced(
        "debounce_skip",
        Duration::from_secs(60)
    ));
    assert!(release_retained_heap_debounced(
        "debounce_run",
        Duration::from_secs(0)
    ));
}

#[test]
fn deferred_request_coalesces_and_drains_once() {
    // Drain any leftover from a sibling test.
    run_deferred_release();
    request_release_after_draw("a");
    request_release_after_draw("b");
    assert!(AFTER_DRAW.load(Ordering::Relaxed));
    run_deferred_release();
    assert!(!AFTER_DRAW.load(Ordering::Relaxed));
    run_deferred_release();
    assert!(!AFTER_DRAW.load(Ordering::Relaxed));
}

#[test]
fn retention_ticker_checks_once_per_interval() {
    let start = Instant::now();
    let mut ticker = RetentionTicker::new(u64::MAX, Duration::from_secs(30), start);
    assert!(!ticker.tick(start + Duration::from_secs(10)), "too soon");
    assert!(
        !ticker.tick(start + Duration::from_secs(31)),
        "checked, nothing to trim"
    );
    assert_eq!(ticker.last, start + Duration::from_secs(31));
    assert!(!ticker.tick(start + Duration::from_secs(40)));
    assert_eq!(ticker.last, start + Duration::from_secs(31), "skipped");
}
