//! Return freed-but-retained allocator pages to the OS from the TUI loop.
//!
//! The trimming itself lives in [`whycodes_core::process_memory`]; this module
//! adds the client's timing: after a turn (deferred past the frame flush), on
//! the edge into idle, again every few minutes while idle, and whenever
//! retained memory grows past a threshold. Inert outside Linux/glibc. Never
//! call on the draw path.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use whycodes_core::process_memory::RetentionWatch;
pub use whycodes_core::process_memory::{release_retained_heap, release_retained_heap_debounced};

/// How long the client must be quiet before an idle trim.
pub const IDLE_TRIM_AFTER: Duration = Duration::from_secs(60);
/// While idle, trim again this often: background ticks slowly regrow the heap.
pub const IDLE_RETRIM_EVERY: Duration = Duration::from_secs(300);
/// How often the client checks for retained memory.
pub const RETENTION_CHECK_EVERY: Duration = Duration::from_secs(30);
/// Retained growth that triggers a trim in a client (several may run at once).
pub const CLIENT_RETENTION_TRIM_BYTES: u64 = 16 * 1024 * 1024;

static AFTER_DRAW: AtomicBool = AtomicBool::new(false);
static AFTER_DRAW_REASON: Mutex<&'static str> = Mutex::new("post-draw");

/// Retention check on a fixed cadence, driven by the event loop's clock.
pub struct RetentionTicker {
    watch: RetentionWatch,
    every: Duration,
    last: Instant,
}

impl RetentionTicker {
    pub fn new(threshold_bytes: u64, every: Duration, now: Instant) -> Self {
        Self {
            watch: RetentionWatch::new(threshold_bytes),
            every,
            last: now,
        }
    }

    /// Check at most once per interval. Returns whether it trimmed.
    pub fn tick(&mut self, now: Instant) -> bool {
        if now.saturating_duration_since(self.last) < self.every {
            return false;
        }
        self.last = now;
        self.watch.check("client_retention")
    }
}

/// Coalesce trims requested on the
/// draw/tick path and run them **after** the frame flush so `malloc_trim`
/// cannot stall the paint the user is waiting for.
pub fn request_release_after_draw(reason: &'static str) {
    if let Ok(mut r) = AFTER_DRAW_REASON.lock() {
        *r = reason;
    }
    AFTER_DRAW.store(true, Ordering::Relaxed);
}

/// Drain a pending [`request_release_after_draw`]. Call once after
/// `terminal.draw` succeeds.
pub fn run_deferred_release() {
    if AFTER_DRAW.swap(false, Ordering::Relaxed) {
        let reason = AFTER_DRAW_REASON.lock().map(|r| *r).unwrap_or("post-draw");
        release_retained_heap(reason);
    }
}

#[cfg(test)]
#[path = "heap_tests.rs"]
mod tests;
