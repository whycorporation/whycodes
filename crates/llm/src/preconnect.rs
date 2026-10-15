//! Open the provider connection while the user is still typing, so the first
//! request skips DNS, TCP and TLS setup.
//!
//! The dial is a plain `GET` of the endpoint's origin through the shared
//! client: no path, no credentials, no prompt. Its pooled connection is what
//! the real request reuses. Each origin is dialled at most once per
//! [`PRECONNECT_WINDOW`]; a failed dial frees the origin for the next try.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// Matches reqwest's default pool idle timeout: a connection older than this
/// may already be gone, so dialling again is worth it.
pub const PRECONNECT_WINDOW: Duration = Duration::from_secs(90);
/// Give up on a dial after this long.
pub const PRECONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Read at most this much of the origin's reply so HTTP/1 can pool the socket.
const DRAIN_CAP_BYTES: usize = 64 * 1024;

fn dialled() -> MutexGuard<'static, HashMap<String, Instant>> {
    static DIALLED: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    DIALLED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// `scheme://host[:port]` of an http(s) URL.
pub fn endpoint_origin(url: &str) -> Option<String> {
    let url = reqwest::Url::parse(url).ok()?;
    if !matches!(url.scheme(), "http" | "https") || !url.has_host() {
        return None;
    }
    Some(url.origin().ascii_serialization())
}

/// Claim `origin` for a dial unless it was dialled within the window.
fn claim(origin: &str, now: Instant) -> bool {
    let mut map = dialled();
    if map
        .get(origin)
        .is_some_and(|at| now.duration_since(*at) < PRECONNECT_WINDOW)
    {
        return false;
    }
    map.retain(|_, at| now.duration_since(*at) < PRECONNECT_WINDOW);
    map.insert(origin.to_string(), now);
    true
}

fn release(origin: &str) {
    dialled().remove(origin);
}

/// Start a background dial to `url`'s origin. Returns whether one started:
/// not for a non-http URL, an origin dialled within the window, or outside a
/// Tokio runtime.
pub fn spawn(url: &str) -> bool {
    let Some(origin) = endpoint_origin(url) else {
        return false;
    };
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return false;
    };
    if !claim(&origin, Instant::now()) {
        return false;
    }
    rt.spawn(async move {
        if !dial(&origin).await {
            release(&origin);
        }
    });
    true
}

/// GET the origin and drain a bounded body; `true` when the reply arrived.
pub(crate) async fn dial(origin: &str) -> bool {
    let request = crate::client_identity::http_client().get(origin).send();
    let drained = async {
        let mut response = request.await?;
        let mut read = 0usize;
        while read <= DRAIN_CAP_BYTES
            && let Some(chunk) = response.chunk().await?
        {
            read += chunk.len();
        }
        Ok::<_, reqwest::Error>(())
    };
    let done = tokio::time::timeout(PRECONNECT_TIMEOUT, drained).await;
    tracing::debug!(origin, ok = matches!(done, Ok(Ok(()))), "preconnect");
    matches!(done, Ok(Ok(())))
}

#[cfg(test)]
#[path = "preconnect_tests.rs"]
mod tests;
