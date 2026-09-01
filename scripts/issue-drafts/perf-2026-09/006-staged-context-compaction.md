# Staged context compaction with budget-aware truncation before LLM summarization

## Problem
`crates/session/src/session.rs:compact` truncates tool results to fixed char caps (`TOOL_RESULT_PRUNE_CHARS` / `TOOL_RESULT_SHAKE_CHARS`) and then optionally asks the model to summarize. The caps are heuristic and uniform — recent high-value tool output (test failures, compiler errors) is pruned just as aggressively as stale file listings, hurting answer quality right when the window is tightest.

## Proposal
Make compaction budget-aware and staged, with a quality-preserving fall-back chain.

**How it works:**
- Stage 1 — deterministic prune: keep the last `N` tool results at full `TOOL_RESULT_MAX_CHARS` (already `PRUNE_KEEP_RECENT_TOOLS=4`), but rank older results by signal (error/diff/test output > directory listing) using a lightweight classifier (substring match on `error|failed|panic|diff --`) before truncating to `TOOL_RESULT_PRUNE_CHARS`.
- Stage 2 — token-budget prune: estimate tokens via `whycodes_core::tokens::estimate_tokens` and evict lowest-signal messages until the session is under `max_context_tokens * 0.75` (not just `MIN_KEEP_MESSAGES`).
- Stage 3 — LLM summarize (existing path): only if still over budget, call the compact model with the `dropped_transcript` window. Preserve the `COMPACT_CONTINUATION_PREAMBLE` and keep the summary under `SUMMARY_TOKEN_SLACK`.
- Expose `CompactOutcome::still_over` / `failed` thresholds so the agent loop can retry compaction at most once per turn (avoid loops).

**Expected benefit:** Fewer "I lost context" regressions near the window limit; higher quality after compaction on long sessions; less over-pruning of recent error output.
