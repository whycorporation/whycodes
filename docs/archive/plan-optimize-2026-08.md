# Plan — System optimization (2026-08)

**Status:** Session A + B done · **Owner:** agent · **Baseline:** `docs/benchmarks.md`

## Context

Ship floor is already strong (`--version` ~1 ms, binary ~12 MB, idle draws ~0).
Next wins are long-session paint, session I/O, clone thrash, and blocking work
on the async path — not more LTO.

## Session A — quick wins (this turn)

| # | Item | Risk | Status |
|---|------|------|--------|
| 1 | `CREATE INDEX` on `messages(session_id)` | Low | ✅ |
| 2 | Transaction around `save_to_db` replace | Low | ✅ |
| 3 | Reuse one SQLite handle (TUI process) | Low | ✅ |
| 4 | Session list: one grouped COUNT | Low | ✅ |
| 5 | Single-pass `cap_tool_text_to` | Low | ✅ |
| 6 | Move tool results (no `results.clone()`) | Low | ✅ |
| 7 | Skip `snapshot_cells` when not selecting | Low | ✅ |
| 8 | Shared HTTP client in `webfetch` | Low | ✅ |
| 9 | `spawn_blocking` for `grep` | Low | ✅ |
| 10 | Parallel MCP connect (`join_all`) | Low | ✅ |
| 11 | `init_logging` prefers env (no extra load when set) | Low | ✅ |
| 12 | Virtualize chat paint (visible messages only) | Med | ✅ |

## Session B — long-session / clone / first paint

| # | Item | Risk | Status |
|---|------|------|--------|
| 1 | Defer MCP + auto-index past first paint | Low | ✅ |
| 2 | Closed-message markdown line cache | Med | ✅ |
| 3 | `LlmRequest` `Arc<[Message]>` + COW intent inject | Med | ✅ |
| 4 | Token estimate cache + ASCII byte fast path | Low | ✅ |

## Measurement after Session B

```bash
cargo test -p whycodes-storage -p whycodes-session -p whycodes-tools -p whycodes-agent --lib
cargo check --workspace
cargo build -p whycodes-cli
# optional: python scripts/bench_startup.py --runs 10
```
