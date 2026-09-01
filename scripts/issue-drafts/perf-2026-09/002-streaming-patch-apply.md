# Incremental patch application while the LLM is still streaming

## Problem
`edit` / `apply_patch` are only executed after the model finishes the entire tool-call JSON. For large or multi-hunk edits the user sees "model is thinking" for seconds while the final diff is already determinable from the streamed delta. Waiting until `tool_end` inflates perceived latency and prevents early conflict detection.

## Proposal
Stream-apply edits incrementally from `ToolCallAssembler` deltas instead of waiting for a complete call.

**How it works:**
- In `crates/agent/src/tool_stream.rs`, expose a `patch_delta` callback that yields the growing `arguments` buffer for `edit`/`apply_patch` calls. Buffer complete hunks (delimited by `@@` or by a closed JSON string field) and validate each hunk against the current file content in a dry-run.
- Paint a provisional diff in the TUI preview pane (`panel` / diff view) as hunks arrive; mark it "provisional" until the call finalizes.
- On finalize, atomically commit the buffered hunks if validation still passes; on mismatch roll back the preview and fall back to the normal full-apply path.
- No filesystem write happens until the full call is validated — the preview is in-memory only.

**Expected benefit:** Time-to-first-visible-edit drops by 30-50% on multi-hunk turns; earlier detection of patch conflicts / bad line numbers.
