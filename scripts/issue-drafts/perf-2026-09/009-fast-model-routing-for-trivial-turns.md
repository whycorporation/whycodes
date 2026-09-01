# Fast-model routing for trivial / mechanical turns

## Problem
Every turn — including mechanical edits (rename, format, import fix, single-hunk patch) — goes to the primary model, even when a smaller, 3-5x lower-latency model would produce an identical patch. This inflates cost and latency on the most common edit shapes.

## Proposal
Route trivial turns to a fast model with automatic fallback to the primary model.

**How it works:**
- In `crates/agent/src/routing.rs` (or a new `crates/llm/src/routing.rs`), classify the user prompt + recent tool results as `trivial` when: single-file edit, hunk count ≤2, no cross-file reasoning, and prompt length < 400 chars with keywords like `rename|fix import|format|typo`. Classification is heuristic-only (no LLM call).
- When `trivial` and `session.model_smol` is configured, send the turn to `model_smol` first. If the fast model returns a tool call that fails validation (bad path / patch conflict) or the edit is rejected by the file-claim check, automatically retry the same turn once on the primary model without user intervention.
- Surface the routing decision as a `status` event (`routed to <model_smol>`) so the TUI header chip reflects the actual model used that turn. Billing still reports per-model `Usage`.

**Expected benefit:** 40-60% lower latency and cost on mechanical edits; primary model reserved for reasoning-heavy turns; no quality regression due to single-retry fallback.
