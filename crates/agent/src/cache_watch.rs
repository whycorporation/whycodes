//! Prompt-cache miss notice: a step re-sent a prompt the previous step had
//! cached (provider TTL expired, prefix changed, model switched).

use whycodes_core::types::Usage;

/// Cache reads below this on the previous step are too small to report.
const MIN_CACHED: u64 = 2_048;

/// Status line when `usage` read far less from the cache than `prev_read`.
///
/// `None` unless the previous step read at least [`MIN_CACHED`] tokens from
/// the cache and this one read under a quarter of that while still sending
/// that much prompt.
pub(crate) fn cache_miss_notice(prev_read: u64, usage: &Usage) -> Option<String> {
    let read = usage.cache_read_input_tokens.unwrap_or(0);
    let sent = usage.input_tokens + usage.cache_creation_input_tokens.unwrap_or(0);
    if prev_read < MIN_CACHED || read >= prev_read / 4 || sent < MIN_CACHED {
        return None;
    }
    Some(format!(
        "Prompt cache miss: {} tokens sent uncached (last step read {} from cache)",
        tokens(sent),
        tokens(prev_read)
    ))
}

fn tokens(n: u64) -> String {
    if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
#[path = "cache_watch_tests.rs"]
mod tests;
