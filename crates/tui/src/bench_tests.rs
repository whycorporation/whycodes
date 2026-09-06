use super::*;

/// The counters are global, so every test that mutates them lives in this
/// single case. Parallel libtest workers would otherwise race on the
/// atomics (seen as a macOS-only flake on CI).
#[test]
fn counters_record_the_first_frame_and_the_total() {
    ENABLED.store(true, Ordering::Relaxed);
    DRAWS.store(0, Ordering::Relaxed);
    FIRST_FRAME_NANOS.store(0, Ordering::Relaxed);

    let config = BenchConfig {
        output: std::path::PathBuf::from("unused"),
        duration: Duration::ZERO,
    };
    assert!(
        !should_stop(&config),
        "with no frame drawn there is nothing to have measured"
    );

    record_draw();
    let first = FIRST_FRAME_NANOS.load(Ordering::Relaxed);
    assert!(first > 0, "the first draw should have been timed");
    assert!(
        should_stop(&config),
        "zero duration means stop as soon as the first frame is up"
    );

    record_draw();
    record_draw();
    assert_eq!(DRAWS.load(Ordering::Relaxed), 3);
    assert_eq!(
        FIRST_FRAME_NANOS.load(Ordering::Relaxed),
        first,
        "a later frame must not overwrite the first"
    );

    ENABLED.store(false, Ordering::Relaxed);
    let before = DRAWS.load(Ordering::Relaxed);
    record_draw();
    assert_eq!(
        DRAWS.load(Ordering::Relaxed),
        before,
        "recording should be inert when disabled"
    );
}

#[test]
fn a_rate_over_no_time_is_zero_rather_than_infinite() {
    assert_eq!(rate(10, Duration::ZERO), 0.0);
    assert_eq!(rate(0, Duration::from_secs(1)), 0.0);
    assert_eq!(rate(50, Duration::from_secs(2)), 25.0);
}

#[test]
fn json_is_parseable_and_carries_every_field() {
    let m = Measurement {
        first_frame_ms: 12.5,
        draws: 100,
        observed_ms: 2000.0,
        draws_per_second: 49.5,
    };
    let parsed: serde_json::Value = serde_json::from_str(&m.to_json()).unwrap();
    assert_eq!(parsed["first_frame_ms"], 12.5);
    assert_eq!(parsed["draws"], 100);
    assert_eq!(parsed["observed_ms"], 2000.0);
    assert_eq!(parsed["draws_per_second"], 49.5);
}

#[test]
fn benchmarking_is_off_without_the_environment_variable() {
    // The variable is not set in the test environment, so this is the
    // normal path: no config, and the loop pays nothing.
    if std::env::var("WHYCODES_BENCH").is_err() {
        assert!(config_from_env().is_none());
    }
}
