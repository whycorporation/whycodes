//! Give retained heap back to the OS from the daemon: after turns, after idle
//! sessions leave memory, and when a periodic check sees retained memory grow. The trimming itself lives in
//! [`whycodes_core::process_memory`]; inert outside Linux/glibc.

use std::time::Duration;

use whycodes_core::process_memory::{
    RetentionWatch, release_retained_heap, release_retained_heap_debounced,
};

/// At most one after-turn trim per this interval across all sessions.
pub const TURN_TRIM_DEBOUNCE: Duration = Duration::from_secs(30);
/// How often the daemon checks for retained memory.
pub const RETENTION_CHECK_EVERY: Duration = Duration::from_secs(120);
/// Retained growth that triggers a trim in the daemon.
pub const RETENTION_TRIM_BYTES: u64 = 64 * 1024 * 1024;

/// Call when a chat or run turn finished.
pub(crate) fn after_turn() {
    release_retained_heap_debounced("server_turn_done", TURN_TRIM_DEBOUNCE);
}

/// Call after an eviction pass; trims when it dropped any session.
pub(crate) fn after_eviction(evicted: usize) {
    if evicted > 0 {
        release_retained_heap("server_sessions_evicted");
    }
}

/// Check retained memory every `every` for the life of the daemon.
pub fn spawn_retention_watchdog(
    every: Duration,
    threshold_bytes: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut watch = RetentionWatch::new(threshold_bytes);
        loop {
            tokio::time::sleep(every).await;
            watch.check("server_retention");
        }
    })
}

#[cfg(test)]
#[path = "heap_tests.rs"]
mod tests;
