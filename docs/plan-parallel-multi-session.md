# Plan: Parallel multi-session

Feature-matrix row: `Parallel multi-session` (docs/FEATURES.md §4) — currently ❌.
Goal: multiple sessions run concurrently in one whycode process; the user
switches between live sessions without waiting for the active turn to finish,
and headless/CLI usage can fan out prompts in parallel.

## Current state (architecture map, 2026-08)

Everything is strictly single-session:

- `crates/tui/src/run.rs` — `run(TuiRunOptions)` creates one `TuiApp`, one
  `Agent`, one `Session`, one `SessionHistory`, one set of mpsc channels
  (`TurnEvent`/`TurnOutcome`/title/catalog/suggest) at lines ~460-474. A turn
  moves `Agent`+`Session` into a `tokio::spawn`ed task via `std::mem::replace`
  and receives them back through `done_tx` as `TurnOutcome::{Ok,Err}`
  (line ~166). `agent_busy`/`cancel_flag`/`turn_join` guard the single
  in-flight turn (lines ~479-485).
- Session switching (`/sessions` picker, `/resume`, `--continue`) only happens
  when idle: persist current, replace `session`, reset history
  (run.rs lines ~816-860). `app.pending_session_id` is applied only when
  `!agent_busy`.
- `crates/storage/src/db.rs` — `Database` wraps one `rusqlite::Connection`;
  not `Send`-shareable across threads.
- `ChannelPermissionPrompter` / `ChannelQuestionPrompter` are shared per TUI
  run; parallel sessions each need their own pair (or session-tagged routing).
- Reusable templates: `Agent::spawn_parallel` (Semaphore + tokio::spawn over
  `SubagentRunner`, fresh in-memory `Session` per worker), swarm machinery,
  `BackgroundRegistry`, and the title channel's `(session_id, title)` tagging
  pattern (run.rs line ~463).

## Design

### TUI: per-session runtime + tabs

1. **`SessionRuntime` struct** (new, in `crates/tui/src/run.rs` or
   `crates/tui/src/session_runtime.rs`): extracts the current single-session
   locals —
   `{ agent, session, history, busy, cancel_flag, turn_join, session_backup,
      event_tx, done_tx, pending_perm_queue, pending_question_queue,
      perm_prompter, question_prompter }`.
2. `run()` holds `Vec<SessionRuntime>` + `active: usize`. The render path uses
   the active runtime; background runtimes keep their turn tasks alive and keep
   receiving events.
3. **Event routing**: each runtime keeps its own `event_rx`/`done_rx`; the main
   loop drains all runtimes (round-robin `try_recv`) and only repaints when the
   active runtime changed, or marks a "dirty/activity" dot on inactive tabs.
   Alternative (heavier): tag every `TurnEvent`/`TurnOutcome` with
   `session_id` over one channel — rejected, per-runtime channels are simpler
   and match the existing move-out/move-back pattern.
4. **Tab UI**: a session tab bar above the scrollback (title + busy spinner +
   unread dot). Keys: `Ctrl+PageDown/PageUp` (or `Alt+]`/`Alt+[`) cycle tabs;
   `Ctrl+N` opens a new empty session tab; `/sessions` picker gains a "live"
   section listing in-memory runtimes above persisted sessions; closing a tab
   (`/close` or `Ctrl+W` when idle) persists and drops the runtime.
5. **Persistence**: one `Database` per `SessionRuntime` (SQLite connections
   are cheap; avoids a global Mutex). Saving stays on the existing
   `save_to_db` path, now per runtime.
6. **Permission/question prompts**: each runtime owns its prompter pair;
   dialogs only ever show for the active tab. Background runtime needing
   approval shows a "!" badge on its tab and its turn waits.

### CLI: parallel fan-out (headless)

7. `whycode generate` accepts repeated prompts or `--prompt-file` lines plus
   `-j/--jobs <N>`: runs N prompts concurrently via
   `Agent::spawn_parallel`-style Semaphore, each with its own `Session`
   (persisted), printing one result envelope per line in `--format json` /
   interleaved-tagged NDJSON in `stream-json`.

## Steps

- [ ] S1: Extract `SessionRuntime` from `run.rs` locals; single-runtime
      behaviour unchanged. Regression guard: existing TUI tests pass, and
      `agent.wire_event_sink(event_tx.clone())` plus title/catalog/suggest
      channels behave identically after extraction (these shared channels are
      the easiest thing to break silently). Catalog/suggest stay global for
      now (model catalog is process-wide); title keeps the existing
      `(session_id, title)` tagging so async titles land on the right runtime.
- [ ] S2: `Vec<SessionRuntime>` + active index; drain-all event loop; tab bar
      render; cycle keys; `Ctrl+N` new session.
- [ ] S3: `/sessions` picker live section; switch-to-live-runtime path
      (no idle wait); tab close.
- [ ] S4: Per-runtime `Database` + permission/question prompters; badge on
      background approval requests.
- [ ] S5: CLI parallel fan-out for `generate` (`-j`). Failure mode: each
      prompt gets its own `result` envelope with `is_error` (per-prompt
      failures never abort siblings); process exit code is non-zero if any
      prompt failed; partial envelopes are always printed for completed
      prompts.
- [ ] S6: Docs — FEATURES.md row to ✅, README keybindings/commands,
      status.md entry.

## Acceptance criteria

- Two sessions in one TUI: start a long turn in tab A, switch to tab B, chat
  normally; tab A shows spinner, completes in background, its output is intact
  when switching back.
- Permission prompt raised by a background tab does not steal the dialog from
  the active tab; tab badge appears instead.
- Restarting whycode: both sessions are persisted and resumable via
  `--resume` / `/sessions`.
- `whycode generate "a" "b" -j 2 --format json` prints two envelopes, two
  distinct `session_id`s.
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D
  warnings`, `cargo test --workspace` all green.

## Risks / notes

- Read docs/KNOWHOW.md before touching the event loop (mouse/event-loop
  return-value pitfalls, /dev/tty, silent exits).
- The `TurnOutcome` move-out/move-back pattern must stay per runtime; never
  share an `Agent` between runtimes (it carries per-session state:
  `cwd_override`, `activated_tools`, `subagent_usage_pending`).
- Memory: each runtime holds a full message history; cap live tabs
  (suggest 8) with a clear error.
