# Prompt-cache prefix stability via deterministic system / tool ordering

## Problem
Provider prompt caches are prefix-matched. Any reordering of the system-prompt blocks or of the `tools` array between consecutive turns invalidates the cache, turning a cache-hit turn into a full cache-miss billed at input-token price. Tool definitions are already sorted, but system-prompt injection order (AGENTS.md, memory, skill descriptions) can vary with file discovery timing, and tool-set changes mid-turn break the prefix.

## Proposal
Make the entire cache prefix byte-stable across turns.

**How it works:**
- In `crates/agent/src/context_files.rs` and the system-prompt assembly path, sort and deduplicate all injected blocks (AGENTS.md, sibling instruction files, MEMORY.md excerpts, skill name+description) by a stable key before concatenation.
- Freeze the `tools` array for the duration of a single user turn: even if `tool_search` activates deferred tools mid-turn, keep the original prefix for cache purposes and append new tools only after the turn boundary (or behind a separate cache breakpoint).
- Keep the existing `cache_control` breakpoints (tools + last system block + latest user message) fixed for the whole turn so every intra-turn `assistant→tool→assistant` round reuses the same prefix. Add a CI assertion that two consecutive `LlmRequest` serializations with identical semantic content are byte-identical.

**Expected benefit:** 40-70% cache hit rate on multi-step turns; lower TTFT and 30-50% input-cost reduction on long sessions.
