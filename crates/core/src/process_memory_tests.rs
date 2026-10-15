use super::*;

const MIB: u64 = 1024 * 1024;

#[test]
fn ints_parse_with_a_default() {
    assert_eq!(parse_int(" 7 ", 4), 7);
    assert_eq!(parse_int("x", 4), 4);
    assert_eq!(parse_int("", 4), 4);
    assert_eq!(env_int("WHYCODES_TEST_PROCESS_MEMORY_UNSET", 9), 9);
}

#[test]
fn configure_and_release_are_safe_to_call() {
    configure_system_allocator();
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
        Duration::ZERO
    ));
}

#[test]
fn rss_anon_parses_from_status() {
    let status = "Name:\twhycodes\nRssAnon:\t   2048 kB\nRssFile:\t 10 kB\n";
    assert_eq!(parse_rss_anon(status), Some(2048 * 1024));
    assert_eq!(parse_rss_anon("Name:\tx\n"), None);
    assert_eq!(parse_rss_anon("RssAnon:\tlots kB\n"), None);
    assert_eq!(parse_rss_anon("RssAnon:\n"), None);
}

#[test]
fn measurements_exist_on_linux_glibc_only() {
    let glibc = cfg!(all(target_os = "linux", target_env = "gnu"));
    assert_eq!(rss_anon_bytes().is_some(), cfg!(target_os = "linux"));
    assert_eq!(retained_bytes().is_some(), glibc);
    let mut watch = RetentionWatch::new(u64::MAX);
    assert!(
        !watch.check("unit_test"),
        "never past an infinite threshold"
    );
    assert_eq!(watch.baseline().is_some(), glibc);
}

#[test]
fn watch_trims_on_growth_past_a_baseline_that_only_moves_down() {
    let mut watch = RetentionWatch::new(16 * MIB);
    let unused: fn() -> Option<u64> = || panic!("no trim expected");

    assert!(!watch.observe(40 * MIB, "t", unused));
    assert_eq!(watch.baseline(), Some(40 * MIB));
    assert!(!watch.observe(30 * MIB, "t", unused), "lower: new baseline");
    assert_eq!(watch.baseline(), Some(30 * MIB));
    assert!(
        !watch.observe(45 * MIB, "t", unused),
        "growth under threshold"
    );

    assert!(watch.observe(46 * MIB, "t", || Some(12 * MIB)));
    assert_eq!(watch.baseline(), Some(12 * MIB), "measured after the trim");

    assert!(watch.observe(28 * MIB, "t", || None));
    assert_eq!(
        watch.baseline(),
        Some(28 * MIB),
        "unmeasurable: keep the level"
    );
}
