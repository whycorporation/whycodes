# Conflict-aware parallel execution for write tools with file-claim analysis

## Problem
Parallel fan-out is currently limited to read-only tools (`is_parallel_safe_tool`). When the model emits two `edit` calls touching different files, they still run serially even though they are independent. Conversely, naively parallelizing all writes would cause races when two calls target the same path.

## Proposal
Allow parallel execution of write tools when their target paths are provably disjoint.

**How it works:**
- In `crates/agent/src/agent.rs:run_batch`, partition the incoming `tool_calls` by target path: extract `path`/`file` argument, canonicalize via `crates/tools/src/file/paths.rs:resolve_path`, and group by absolute path.
- Calls targeting distinct files run via `futures::future::join_all`; calls sharing a file (or where the path cannot be extracted) stay serial within that group, preserving existing file-claim checks for parallel TUI sessions (`Ctrl+N`).
- Extend `crates/tools/src/executor.rs` to expose a `target_path(&ToolCall) -> Option<PathBuf>` helper so the agent does not re-parse tool-specific JSON.
- Keep `bash`/`shell` always serial (side effects cannot be proven disjoint).

**Expected benefit:** Multi-file refactors (e.g. rename across 3 files) complete with one round-trip instead of N sequential rounds; wall-clock improves linearly with file count when edits are independent.
