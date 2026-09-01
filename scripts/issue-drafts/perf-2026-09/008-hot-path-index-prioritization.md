# Hot-path prioritization and semantic ranking for the workspace file index

## Problem
`whycodes-index` walks the workspace in filesystem order and serves fuzzy queries via `nucleo` in insertion order. On large repos (200k entries) the files the user actually needs (`src/**`, recently edited paths, `@`-picker prefix matches) can rank below generated/vendor-adjacent files, and the initial scan competes with TUI first-frame for I/O.

## Proposal
Prioritize hot paths during walk and rank results by recency + semantic signal.

**How it works:**
- In `crates/index/src/walk.rs`, walk hot prefixes first (`src/`, `crates/`, `app/`, `lib/`, `packages/`) before the full `ignore`-crate walk, so `@main` / `Ctrl+Space` queries return useful hits while `STATE_SCANNING` is still in progress.
- In `crates/index/src/fuzzy.rs`, boost scores for: recently modified files (mtime within 7 days, via `notify` watcher deltas), files already referenced in the current session transcript, and exact basename matches (`main.rs` scores above `z_main.rs.bak`). Keep `nucleo` as the engine — just adjust the pre-filter ranking before feeding it.
- In `crates/index/src/watch.rs`, increase `WATCH_DEBOUNCE` coalescing for build-artifact storms (`target/`, `node_modules/`) so the index thread is not woken on every compiler write; keep the existing `policy.rs` pruning.

**Expected benefit:** `@`-picker hits the right file in the top 3 on repos >50k files; fewer wasted walks; TUI first-frame not blocked by index I/O.
