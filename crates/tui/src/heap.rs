//! Return freed-but-retained allocator pages to the OS.
//!
//! jcode (`process_memory::release_retained_heap`): after a turn the
//! transcript/layout caches drop large transients, but glibc keeps the
//! arena. `malloc_trim(0)` walks free chunks and gives the pages back.
//! Inert on non-Linux. Never call on the draw path.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long the client must be quiet before an idle trim (jcode: 60s).
pub const IDLE_TRIM_AFTER: Duration = Duration::from_secs(60);

static LAST_TRIM: Mutex<Option<Instant>> = Mutex::new(None);

/// Ask glibc to return unused arena pages. Safe no-op elsewhere.
pub fn release_retained_heap(reason: &'static str) {
    #[cfg(target_os = "linux")]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> i32;
        }
        // SAFETY: malloc_trim is a documented glibc extension; pad=0 means
        // "keep no padding". Other libcs typically lack the symbol — we
        // only compile this block on Linux.
        let trimmed = unsafe { malloc_trim(0) };
        tracing::debug!(reason, trimmed, "malloc_trim");
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = reason;
    }
    if let Ok(mut g) = LAST_TRIM.lock() {
        *g = Some(Instant::now());
    }
}

/// [`release_retained_heap`] unless one already ran inside `min_interval`.
pub fn release_retained_heap_debounced(reason: &'static str, min_interval: Duration) -> bool {
    if let Ok(g) = LAST_TRIM.lock()
        && let Some(last) = *g
        && last.elapsed() < min_interval
    {
        return false;
    }
    release_retained_heap(reason);
    true
}

#[cfg(test)]
mod tests {
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
    }
}
