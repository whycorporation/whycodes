# Speculative execution for read-only tools beyond `read` (grep / glob / list)

## Problem
Speculative early I/O currently covers only the `read` tool. When the model chains several read-only calls (e.g. `grep` → `read` → `glob`), each step still waits for the full `tool_call` JSON to finish streaming before any I/O starts. For `grep`/`glob`/`list`, the key argument (`pattern` / `path`) becomes a closed string just as early as `read.path`, so 100-300ms per turn is left on the table in exploration-heavy turns.

## Proposal
Generalize `crates/agent/src/speculative_read.rs` into a bounded speculative executor for all side-effect-free tools.

**How it works:**
- Watch the partial JSON buffer inside `ToolCallAssembler` while the LLM stream is in flight. As soon as `pattern` (grep/glob) or `path` (list) is a complete quoted string, spawn a `JoinHandle` for that tool call keyed by `call_id`.
- Only side-effect-free tools are eligible (`read`, `grep`, `glob`, `list`); `bash`/`edit`/`write` are explicitly excluded.
- At finalize time, compare the speculated arguments to the parsed `ToolCall.arguments` byte-for-byte. On mismatch discard the speculative result; on match reuse it directly without re-executing.
- Cap concurrency to 2-3 in-flight speculative jobs to bound fd/memory pressure.

**Expected benefit:** 15-25% wall-clock reduction on discovery turns; noticeably shorter "running tool" phase in the TUI.
