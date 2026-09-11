# Windows first-frame harness is ~28× slower than the 2026-07-31 baseline

## Problem

README performance table (Windows 2026-09-11 vs Linux 2026-09-02) looks like a 10× drop, but most of that is OS + harness (console inherit vs 80×24 PTY). The **real** regression is same-OS:

| | 2026-07-31 Windows | 2026-09-11 Windows |
|---|---|---|
| `--version` | 20.9 ms | **13.8 ms** (improved) |
| First frame (in-proc) | **4.74 ms** | **131 ms** |
| Idle redraws / 3 s | 1.96 /s | 0.6 /s |

`--version` is the process-start floor (~14 ms on this box). First frame is **~117 ms after `main`**, before any paint. Linux in-proc TTFF on 2026-09-02 is ~12 ms.

## Why (boot still does work before `record_draw`)

Issue #49 deferred syntect / SQLite memory / plugins / index **after** first paint, but `whycodes run` still pays these **before** the first `terminal.draw`:

1. **`git` spawn on empty projects.** `refresh_git_branch` falls back to `git rev-parse` when `.git/HEAD` is missing. The first-frame harness uses an empty temp dir (no `.git`). Windows `CreateProcess` for a failing `git` is tens of ms.
2. **`Agent::new` + `ToolExecutor::new`** registers ~40 tools before any pixel.
3. **`SessionRuntime::new` opens SQLite** (`whycodes.db`) before paint.
4. **`maybe_offer_import`** walks `$HOME` for other agent CLIs **on the first loop iteration, before draw**.
5. **`config.load_command_files`** walks three command dirs before `tui::run`.
6. **`tui_available()`** opens `CONOUT$` even when stdout is already a TTY; attach opens it again.

## Proposal

Paint chrome first, hydrate the rest (same pattern as #49, one step earlier):

- Split `prepare_tui_boot` into cheap **chrome** (status, provider/model labels, empty file index) vs **runtime** (Agent, Session, SQLite, resume/remote).
- `run()`: attach → first `draw` / `record_draw` → then runtime + `hydrate_after_first_frame`.
- Do not spawn `git` unless `.git` exists; chrome uses the `.git/HEAD` fast path only.
- Skip import scan until after first paint (and when `WHYCODES_BENCH` is set).
- Defer `load_command_files` until hydrate on the TUI path.
- Skip the `CONOUT$` probe when stdout is already a TTY (or `--plain`).

## Acceptance

- Empty-project harness (`scripts/bench_first_frame.py --idle-ms 0`) on Windows: in-proc first frame **well under 50 ms** (target band: process-start floor + paint, not Agent/SQLite/`git`).
- Linux PTY row stays in the ~12 ms band (no regression).
- Idle redraws stay near 0 /s.
- Resume still shows transcript; home import offer still appears **after** first paint.
- `cargo test -p whycodes-tui --lib` and `cargo test -p whycodes-cli --lib` green.

## Non-goals

- Matching Linux 12 ms on Windows (no stdlib ConPTY; console inherit is a different band).
- Multi-session PSS on Windows (`/proc` only).
