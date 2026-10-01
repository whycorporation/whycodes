//! Offload synchronous FS / process work from Tokio worker threads.
//!
//! File tools (`read` / `glob` / `list` / `edit` / …) and `git_*` wrappers
//! call `std::fs` / `Command::output`. Running those on the runtime worker
//! starves stream drain and permission UI when several read-only tools
//! fan out. `grep` already used `spawn_blocking`; the rest of the FS surface
//! now goes through this helper.

use whycodes_core::types::ToolResult;

fn background_task_error(e: impl std::fmt::Display) -> String {
    format!("background task failed: {e}")
}

/// Run `f` on the blocking pool. `Err` is a join failure (panic / cancel).
pub async fn run<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(background_task_error)
}

/// Like [`run`] but maps join failure onto an error [`ToolResult`].
pub async fn tool<F>(f: F) -> ToolResult
where
    F: FnOnce() -> ToolResult + Send + 'static,
{
    match run(f).await {
        Ok(result) => finished_tool(result),
        Err(e) => ToolResult {
            tool_call_id: String::new(),
            content: format!("Error: {e}"),
            is_error: true,
        },
    }
}

fn finished_tool(result: ToolResult) -> ToolResult {
    result
}

#[cfg(test)]
#[path = "blocking_tests.rs"]
mod tests;
