use super::*;
use crate::tests::EnvVarGuard;

/// Exclusive use of the bench globals for one test.
///
/// The counters, `ENABLED` and the `WHYCODES_BENCH*` variables are process
/// state shared with every headless `run()` test (those call `record_draw`
/// and `config_from_env`). Holding `ENV_LOCK` serializes them all. On drop —
/// also when an assert fails — the variables come back and recording is
/// switched off; a leaked `WHYCODES_BENCH` would turn every later `run()`
/// into a splash-and-quit benchmark run.
struct BenchSandbox {
    _bench: EnvVarGuard,
    _duration: EnvVarGuard,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl BenchSandbox {
    fn new() -> Self {
        let lock = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        Self {
            _bench: EnvVarGuard::remove("WHYCODES_BENCH"),
            _duration: EnvVarGuard::remove("WHYCODES_BENCH_DURATION_MS"),
            _lock: lock,
        }
    }
}

impl Drop for BenchSandbox {
    /// Runs before the fields drop, so `ENV_LOCK` is still held.
    fn drop(&mut self) {
        ENABLED.store(false, Ordering::Relaxed);
    }
}

/// The counters are global, so every test that mutates them lives in this
/// single case and holds the sandbox lock.
#[test]
fn counters_record_the_first_frame_and_the_total() {
    let _sandbox = BenchSandbox::new();
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
    let _sandbox = BenchSandbox::new();
    assert!(config_from_env().is_none());
    let _empty = EnvVarGuard::set("WHYCODES_BENCH", "");
    assert!(config_from_env().is_none());
}

#[test]
fn config_from_env_writes_results() {
    let _sandbox = BenchSandbox::new();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("bench.json");
    let _bench = EnvVarGuard::set("WHYCODES_BENCH", &out);
    let _duration = EnvVarGuard::set("WHYCODES_BENCH_DURATION_MS", "not-a-number");
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
}
