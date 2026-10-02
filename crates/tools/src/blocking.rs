//! Offload synchronous FS / process work from Tokio worker threads.
//!
//! File tools (`read` / `glob` / `list` / `edit` / …) and `git_*` wrappers
//! call `std::fs` / `Command::output`. Running those on the runtime worker
//! starves stream drain and permission UI when several read-only tools
//! fan out. `grep` already used `spawn_blocking`; the rest of the FS surface
//! now goes through this helper.

use whycodes_core::types::ToolResult;

fn background_task_error(e: &str) -> String {
    format!("background task failed: {e}")
}

fn join_value(result: Result<ToolResult, tokio::task::JoinError>) -> ToolResult {
    match result {
        Ok(value) => value,
        Err(e) => tool_join_error(&background_task_error(&e.to_string())),
    }
}

/// Run `f` on the blocking pool and map a join failure onto an error [`ToolResult`].
///
/// The job is a trait object so this body is one concrete function. A generic
/// `impl FnOnce` is dropped by `llvm-cov -skip-expansions` when the kept
/// monomorphization comes from an ignored `*_tests.rs` call.
pub async fn tool(f: Box<dyn FnOnce() -> ToolResult + Send>) -> ToolResult {
    // `f` is already `FnOnce`. Clippy wants `spawn_blocking(f)`; that form
    // is a generic monomorphization `-skip-expansions` drops when the only
    // other instantiation lives in an ignored `*_tests.rs`.
    #[allow(clippy::redundant_closure)]
    let joined = tokio::task::spawn_blocking(move || f()).await;
    join_value(joined)
}

fn tool_join_error(e: &str) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: format!("Error: {e}"),
        is_error: true,
    }
}

#[cfg(test)]
#[path = "blocking_tests.rs"]
mod tests;
