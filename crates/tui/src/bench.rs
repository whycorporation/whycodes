//! Instrumentation for measuring the render loop from outside.
//!
//! Time to first frame is the number that decides whether a TUI feels
//! instant, and it cannot be measured by spawning a process and timing it: a
//! process that has exited tells you nothing about when it drew. Neither can
//! idle redraws — a loop that repaints when nothing changed burns CPU
//! invisibly, and the only way to see it is to count.
//!
//! So the loop reports on itself. Setting `WHYCODE_BENCH` to a file path turns
//! this on; everything here is inert otherwise, and the checks are two atomic
//! loads per frame.
//!
//! ```text
//! WHYCODE_BENCH=/tmp/out.json                  exit as soon as the first frame is up
//! WHYCODE_BENCH_DURATION_MS=2000               … or keep drawing for 2s and count
//! ```
//!
//! The file it writes holds the in-process split. The harness measures spawn to
//! file, so the difference between the two is process start and dynamic
//! linking — cost a user pays but this process cannot see.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// When the process started. Set by `main` before anything else runs.
static PROCESS_START: OnceLock<Instant> = OnceLock::new();

/// Call once, as the first statement of `main`.
///
/// Not `Instant::now()` inside the TUI: by then config loading and provider
/// setup have already happened, and those are part of what a user waits for.
pub fn mark_process_start() {
    let _ = PROCESS_START.set(Instant::now());
}

fn process_start() -> Instant {
    *PROCESS_START.get_or_init(Instant::now)
}

/// Counters for one run. A single global rather than threaded through the loop:
/// this exists to be absent in normal use, and a parameter on every call site
/// would not be.
static DRAWS: AtomicU64 = AtomicU64::new(0);
static FIRST_FRAME_NANOS: AtomicU64 = AtomicU64::new(0);
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Where to write results, and how long to keep drawing after the first frame.
pub struct BenchConfig {
    pub output: std::path::PathBuf,
    pub duration: Duration,
}

/// Read the environment. `None` when benchmarking is off, which is the normal
/// case.
pub fn config_from_env() -> Option<BenchConfig> {
    let output = std::env::var("WHYCODE_BENCH")
        .ok()
        .filter(|s| !s.is_empty())?;
    let duration = std::env::var("WHYCODE_BENCH_DURATION_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or_default();
    ENABLED.store(true, Ordering::Relaxed);
    Some(BenchConfig {
        output: std::path::PathBuf::from(output),
        duration,
    })
}

/// Record that a frame was drawn. Called immediately after every `draw`.
pub fn record_draw() {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    DRAWS.fetch_add(1, Ordering::Relaxed);
    // `compare_exchange` rather than a check-then-set: the first frame is the
    // one being timed, and it must not be overwritten by the second.
    let elapsed = process_start().elapsed().as_nanos() as u64;
    let _ = FIRST_FRAME_NANOS.compare_exchange(0, elapsed, Ordering::Relaxed, Ordering::Relaxed);
}

/// True once the run has drawn its first frame and outstayed its duration.
pub fn should_stop(config: &BenchConfig) -> bool {
    let first = FIRST_FRAME_NANOS.load(Ordering::Relaxed);
    if first == 0 {
        return false;
    }
    let since_first = process_start().elapsed() - Duration::from_nanos(first);
    since_first >= config.duration
}

/// What a run measured.
#[derive(Debug, Clone, PartialEq)]
pub struct Measurement {
    pub first_frame_ms: f64,
    pub draws: u64,
    pub observed_ms: f64,
    pub draws_per_second: f64,
}

/// Read the counters and compute the derived figures.
pub fn measure() -> Measurement {
    let first_nanos = FIRST_FRAME_NANOS.load(Ordering::Relaxed);
    let draws = DRAWS.load(Ordering::Relaxed);
    let total = process_start().elapsed();
    let observed = total.saturating_sub(Duration::from_nanos(first_nanos));

    Measurement {
        first_frame_ms: first_nanos as f64 / 1e6,
        draws,
        observed_ms: observed.as_secs_f64() * 1000.0,
        // Draws after the first, over the time after the first: the rate the
        // loop settles at, not one inflated by the startup frame.
        draws_per_second: rate(draws.saturating_sub(1), observed),
    }
}

fn rate(count: u64, over: Duration) -> f64 {
    let seconds = over.as_secs_f64();
    if seconds <= 0.0 {
        0.0
    } else {
        count as f64 / seconds
    }
}

impl Measurement {
    pub fn to_json(&self) -> String {
        format!(
            "{{\n  \"first_frame_ms\": {:.3},\n  \"draws\": {},\n  \"observed_ms\": {:.3},\n  \"draws_per_second\": {:.2}\n}}\n",
            self.first_frame_ms, self.draws, self.observed_ms, self.draws_per_second
        )
    }
}

/// Write the results where the harness will look for them.
///
/// Called after the terminal has been restored, so a write failure cannot
/// corrupt the screen. The harness treats a missing file as a failed run, which
/// is the right reading: no file means no first frame.
pub fn write_results(config: &BenchConfig) {
    let measurement = measure();
    if let Err(e) = std::fs::write(&config.output, measurement.to_json()) {
        eprintln!(
            "whycode: could not write benchmark results to {}: {e}",
            config.output.display()
        );
    }
}

#[cfg(test)]
mod tests {
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
        if std::env::var("WHYCODE_BENCH").is_err() {
            assert!(config_from_env().is_none());
        }
    }
}
