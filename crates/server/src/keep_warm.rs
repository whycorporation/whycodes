//! Opt-in prompt-cache keep-alive for idle daemon sessions
//! (`session.cache_warm_minutes`), the daemon side of the TUI keep-alive.
//!
//! After a turn, a task re-sends the session's last request every few
//! minutes while the session stays idle, so its cached prefix does not
//! expire. It stops when the idle window is over, when another turn starts
//! (the session is locked or its history grew), or when there is nothing to
//! keep warm.

use std::sync::Arc;
use std::time::{Duration, Instant};

use whycodes_agent::Agent;

use crate::SessionHandle;

/// Provider, model and API key the last turn used.
pub(crate) type Route = (String, String, String);

/// Start keeping `handle` warm for `window_minutes` of idleness; `None`
/// when keep-alive is off or another turn already holds the session.
pub(crate) fn spawn(
    agent: Arc<Agent>,
    handle: SessionHandle,
    route: Route,
    window_minutes: u64,
) -> Option<tokio::task::JoinHandle<u32>> {
    let window = Duration::from_secs(window_minutes.saturating_mul(60));
    let every = whycodes_agent::agent::cache_warm::WARM_EVERY;
    spawn_every(agent, handle, route, window, every)
}

pub(crate) fn spawn_every(
    agent: Arc<Agent>,
    handle: SessionHandle,
    route: Route,
    window: Duration,
    every: Duration,
) -> Option<tokio::task::JoinHandle<u32>> {
    if window.is_zero() {
        return None;
    }
    let history = handle.try_lock().ok()?.messages.len();
    let task = run(agent, handle, history, route, window, every);
    Some(tokio::spawn(task))
}

/// Returns how many keep-alives were recorded.
async fn run(
    agent: Arc<Agent>,
    handle: SessionHandle,
    history: usize,
    (provider, model, api_key): Route,
    window: Duration,
    every: Duration,
) -> u32 {
    let ended = Instant::now();
    let mut sent = 0;
    loop {
        tokio::time::sleep(every).await;
        if ended.elapsed() >= window {
            return sent;
        }
        let job = {
            let Ok(session) = handle.try_lock() else {
                return sent;
            };
            if session.messages.len() != history {
                return sent;
            }
            match agent.cache_warm_job(&session, &provider, &model, &api_key) {
                Some(job) => job,
                None => return sent,
            }
        };
        let Some(usage) = job.await else {
            continue;
        };
        let mut session = handle.lock().await;
        if session.messages.len() != history {
            return sent;
        }
        agent.record_cache_warm(&mut session, &provider, &model, &usage);
        let saved = crate::routes::save_session(&session);
        crate::routes::warn_persist("serve: failed to save session after keep-alive", saved);
        sent += 1;
    }
}

#[cfg(test)]
#[path = "keep_warm_tests.rs"]
mod tests;
