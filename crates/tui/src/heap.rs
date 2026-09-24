//! Return freed-but-retained allocator pages to the OS.
//!
//! jcode (`process_memory::release_retained_heap`): after a turn the
//! transcript/layout caches drop large transients, but glibc keeps the
//! arena. `malloc_trim(0)` walks free chunks and gives the pages back.
//! Inert on non-Linux. Never call on the draw path.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long the client must be quiet before an idle trim (jcode: 60s).
pub const IDLE_TRIM_AFTER: Duration = Duration::from_secs(60);

static LAST_TRIM: Mutex<Option<Instant>> = Mutex::new(None);
static AFTER_DRAW: AtomicBool = AtomicBool::new(false);
static AFTER_DRAW_REASON: Mutex<&'static str> = Mutex::new("post-draw");

/// Ask glibc to return unused arena pages. Safe no-op elsewhere.
pub fn release_retained_heap(reason: &'static str) {
    release_retained_heap_on(cfg!(target_os = "linux"), reason);
}

/// Host gate extracted so Linux skip-expansions can drive both arms.
fn release_retained_heap_on(is_linux: bool, reason: &'static str) {
    if is_linux {
        malloc_trim_linux(reason);
    } else {
        let _ = reason;
    }
    if let Ok(mut g) = LAST_TRIM.lock() {
        *g = Some(Instant::now());
    }
}

#[cfg(target_os = "linux")]
fn malloc_trim_linux(reason: &'static str) {
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
fn malloc_trim_linux(_reason: &'static str) {}

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

/// Grok `request_release_after_draw`: coalesce trims requested on the
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
