//! MCP servers connect in the background so the first frame and the event
//! loop never wait on a slow or hanging server. A turn or `/mcp` waits a
//! short while for them; a slower server's tools join a later turn.

use std::time::Duration;

use tokio::task::JoinHandle;
use whycodes_agent::agent::McpLoad;
use whycodes_config::Config;

use crate::app::TuiApp;
use crate::session_runtime::SessionRuntime;

/// Longest a turn or `/mcp` waits for servers still connecting. The load
/// itself may take up to a minute (`npx -y` downloads), and a frozen prompt
/// for that long is worse than a first turn without those tools.
pub(super) const TURN_WAIT: Duration = Duration::from_secs(5);

/// A spawned [`whycodes_agent::agent::Agent::begin_mcp_load`], if any.
#[derive(Default)]
pub(super) struct PendingMcp(Option<JoinHandle<McpLoad>>);

impl PendingMcp {
    /// Load plugins now; connect configured MCP servers in a task.
    pub(super) async fn start(rt: &mut SessionRuntime, config: &Config) -> Self {
        if config.mcp_servers.is_empty() {
            rt.agent.load_mcp(config).await;
            return Self(None);
        }
        Self(Some(tokio::spawn(rt.agent.begin_mcp_load(config))))
    }

    /// Install a finished load once no session has its agent out on a turn.
    pub(super) fn poll(&mut self, rt: &mut SessionRuntime, runtimes: &mut [SessionRuntime]) {
        let ready = self.0.as_ref().is_some_and(JoinHandle::is_finished);
        if !ready || rt.agent_busy || runtimes.iter().any(|r| r.agent_busy) {
            return;
        }
        if let Some(joined) = self.0.take().and_then(futures::FutureExt::now_or_never) {
            install(joined, rt, runtimes);
        }
    }

    /// Wait up to `limit` for the servers before a turn or `/mcp` needs
    /// them. Still connecting after that: tell the user and let [`Self::poll`]
    /// install them later.
    pub(super) async fn wait(
        &mut self,
        app: &mut TuiApp,
        rt: &mut SessionRuntime,
        runtimes: &mut [SessionRuntime],
        limit: Duration,
    ) {
        let Some(handle) = self.0.as_mut() else {
            return;
        };
        match tokio::time::timeout(limit, handle).await {
            Ok(joined) => {
                self.0 = None;
                install(joined, rt, runtimes);
            }
            Err(elapsed) => {
                tracing::debug!(%elapsed, "MCP servers still connecting");
                app.toasts.push(
                    crate::toast::ToastKind::Info,
                    "MCP servers still connecting — their tools join a later turn",
                );
            }
        }
    }
}

/// `/mcp` and `/mcp__<server>__<prompt>` read the connected servers.
pub(super) fn reads_mcp_servers(slash: &str) -> bool {
    slash == "/mcp" || slash.starts_with("/mcp ") || slash.starts_with("/mcp__")
}

/// Sessions share one tool set, so every idle session gets the load.
fn install(
    joined: Result<McpLoad, tokio::task::JoinError>,
    rt: &mut SessionRuntime,
    runtimes: &mut [SessionRuntime],
) {
    let loaded = match joined {
        Ok(loaded) => loaded,
        Err(e) => {
            tracing::warn!(error = %e, "MCP load task failed");
            return;
        }
    };
    for r in std::iter::once(rt).chain(runtimes.iter_mut()) {
        if !r.agent_busy {
            r.agent.finish_mcp_load(loaded.clone());
        }
    }
}

#[cfg(test)]
#[path = "mcp_defer_tests.rs"]
mod tests;
