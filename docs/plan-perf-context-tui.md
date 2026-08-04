# Plan — Context economy + TUI paint path

**Status:** done · **Depends on:** [plan-perf-hotpath.md](plan-perf-hotpath.md) (done) · **Related:** [plan-performance.md](plan-performance.md), [benchmarks.md](benchmarks.md)

## Diagnosis

Hot paths (highlight warm, command-risk, first frame, release LTO) are already
good. Remaining cost is elsewhere:

| Area | Finding | Action |
|------|---------|--------|
| **Context / tokens** | `Session::compact` keeps a fixed last-4 messages; `max_tokens` is unused (`_target`). Tool dumps stay full size forever. | Token-budget compact + cap oversized tool results on ingest |
| **TUI layout** | `message_row_layout` re-runs full `render_message` for every message on scroll/selection, and paint does it again | Per-message height cache keyed by width (+ busy state) |
| **Idle draws** | Adaptive poll cut 21 → ~2 draw/s; still not zero | Dirty-flag draw: skip paint when nothing changed |
| **Streaming** | Each `TextDelta` is applied separately before the next draw | Coalesce queued deltas into one append per loop iteration |

## Non-goals (this plan)

- Feature-gating binary / slimming two-face (separate size pass)
- Full LTO, `build_request` Arc sharing, usage DB migration
- CI first-frame gate (still residual in plan-performance)
- LLM-authored summaries for compact (stub + role tally is enough for now)

## Tasks

1. [x] Token-aware `Session::compact` + improved `token_count` heuristic
2. [x] Cap tool result bodies on agent ingest + during compact
3. [x] Chat message layout height cache
4. [x] Dirty-flag draw loop + toast prune reports change
5. [x] Coalesce stream text/thinking deltas per drain
6. [x] Tests + `cargo test` / check for touched crates
7. [x] Update [benchmarks.md](benchmarks.md) / [status.md](status.md); commit + push

## Acceptance

- Compact under a small budget drops oldest messages until `token_count() ≤ ¾·max` while keeping at least 4 tail messages (or all if fewer).
- Messages already under budget are not reshuffled (tool-cap only).
- Oversized tool results (`> 32_000` chars) are truncated with a clear marker.
- Scroll/`session_line_count` hits cache when width and content are unchanged.
- Idle TUI with no input/toasts/stream: **0 draws/s** after first frame (bench path).
- Streaming still feels continuous; one draw applies a batch of deltas.

## Risks

- **Missed dirty**: stale screen if a mutation forgets `mark_dirty`. Mitigate by
  dirtying on every channel drain and on any input event; animation paths
  (busy spinner, live toasts) keep the flag set.
- **Height cache vs selection**: selection caret does not change row count;
  busy→idle adds epilogue rows — cache key includes `is_busy()`.
- **Heuristic tokens ≠ provider BPE**: same family of estimate as today; good
  enough for compaction thresholds. Provider `Usage` remains the meter for UI.
