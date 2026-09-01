# Streaming tool output in the TUI (incremental `tool_end` rendering)

## Problem
Tool results appear in the TUI only after `tool_end`. For long-running `bash` commands, large `grep` results, or `read` of big files, the user sees a spinner for seconds with no feedback. Failures are also only surfaced at the end, delaying cancellation.

## Proposal
Stream tool output incrementally into the scrollback while the tool is still running.

**How it works:**
- In `crates/tools/src/shell.rs` and `crates/tools/src/grep.rs`, change the tool trait to optionally emit `StreamEvent::ToolDelta` chunks (stdout/stderr fragments) via the existing `EventSink` / `TurnEvent` channel. The agent loop forwards these as `tool_delta` events to the TUI.
- In `crates/tui/src/run.rs`, render `tool_delta` as an appended, collapsible block under the in-flight tool entry (same styling as the final `tool_end` block) with a live line counter. On `tool_end`, replace the streaming block atomically with the final truncated result (`TOOL_RESULT_MAX_CHARS`) to avoid duplication.
- Keep the existing `TurnEvent::ToolEnd` as the source of truth for the session transcript; streaming blocks are view-only and not persisted until finalized. Long output is still truncated at `TOOL_RESULT_MAX_CHARS` on finalize.

**Expected benefit:** Perceived latency drops for shell/test/lint tools; users can `Esc` to cancel earlier when they see the right (or wrong) output streaming.
