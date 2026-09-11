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
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var_os("WHYCODES_BENCH");
    let prev_dur = std::env::var_os("WHYCODES_BENCH_DURATION_MS");
    unsafe {
        std::env::remove_var("WHYCODES_BENCH");
        std::env::remove_var("WHYCODES_BENCH_DURATION_MS");
    }
    assert!(config_from_env().is_none());
    unsafe {
        std::env::set_var("WHYCODES_BENCH", "");
    }
    assert!(config_from_env().is_none());
    restore_env("WHYCODES_BENCH", prev);
    restore_env("WHYCODES_BENCH_DURATION_MS", prev_dur);
}

#[test]
fn config_from_env_writes_results() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var_os("WHYCODES_BENCH");
    let prev_dur = std::env::var_os("WHYCODES_BENCH_DURATION_MS");
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("bench.json");
    unsafe {
        std::env::set_var("WHYCODES_BENCH", &out);
        std::env::set_var("WHYCODES_BENCH_DURATION_MS", "not-a-number");
    }
    let cfg = config_from_env().expect("path is set");
    assert_eq!(cfg.output, out);
    assert_eq!(cfg.duration, Duration::ZERO);
    mark_process_start();
    ENABLED.store(true, Ordering::Relaxed);
    DRAWS.store(0, Ordering::Relaxed);
    FIRST_FRAME_NANOS.store(0, Ordering::Relaxed);
    record_draw();
    write_results(&cfg);
    let json = std::fs::read_to_string(&out).unwrap();
    assert!(json.contains("first_frame_ms"), "{json}");
    let m = measure();
    assert!(m.draws >= 1);
    write_results(&BenchConfig {
        output: dir.path().join("missing").join("out.json"),
        duration: Duration::ZERO,
    });
    ENABLED.store(false, Ordering::Relaxed);
    restore_env("WHYCODES_BENCH", prev);
    restore_env("WHYCODES_BENCH_DURATION_MS", prev_dur);
}

fn restore_env(key: &str, prev: Option<std::ffi::OsString>) {
    unsafe {
        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
}
