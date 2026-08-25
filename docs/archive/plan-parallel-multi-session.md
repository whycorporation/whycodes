# Plan: Parallel multi-session

Feature-matrix row: `Parallel multi-session` (docs/archive/features.md §4) — currently ❌.
Goal: multiple sessions run concurrently in one whycodes process; the user
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

### TUI: per-session runtime + dashboard (revised 2026-08-08)

Competitive research (Claude Code Agent View, Codex `/agent`, OpenCode
session-list, Gemini CLI) shows **no first-party CLI ships a tab bar**:
tabs were proposed in OpenCode (#12548, PR #17984) and stalled; what
shipped everywhere is a **live-status picker/dashboard** + fast cycle keys.
Splits are universally delegated to tmux/Zellij. Design revised accordingly:

1. **`SessionRuntime` struct** (`crates/tui/src/session_runtime.rs`):
   extracts the single-session locals —
   `{ agent, session, history, busy, cancel_flag, turn_join, session_backup,
     event_tx, done_tx, pending_perm_queue, pending_question_queue,
     perm_prompter, question_prompter }`.
2. **Per-session state machine** — `working / waiting_permission /
   waiting_input / idle / done / error` + `unread` flag. This drives every
   visual (dashboard grouping, cycle-key order, badges); the widget choice
   stays a thin rendering decision on top.
3. `run()` holds `Vec<SessionRuntime>` + `active: usize` (+ MRU stack).
   The render path uses the active runtime; background runtimes keep their
   turn tasks alive and keep receiving events.
4. **Event routing**: each runtime keeps its own `event_rx`/`done_rx`; the
   main loop drains all runtimes (round-robin `try_recv`). Events on the
   active runtime repaint; events on inactive runtimes set `unread` and
   update state only. Per-runtime channels (not session-tagged single
   channel) — matches the existing move-out/move-back pattern.
5. **Dashboard overlay** (Claude Agent View-style, the primary surface):
   rows grouped `Needs input → Working → Idle/Done`; each row = title,
   state glyph/spinner, one-line last-activity preview, age. `Ctrl+O`
   opens; `Enter` attaches; `Esc`/`←` detaches; `Space` peeks the last
   lines. Permission/question prompts never steal the active session's
   dialog — a background session needing approval shows `!` in the
   dashboard and its turn waits.
6. **Cycle keys**: `Ctrl+PageDown/PageUp` cycles live sessions in order;
   `Ctrl+Tab` MRU-switches; `Ctrl+N` opens a new empty session.
   No persistent tab strip by default (vertical space); a config-gated
   1-row strip may follow if users ask.
7. **View state**: each runtime carries its own transcript view
   (messages, scroll offset, input draft) so switching is lossless.
8. **Persistence**: one `Database` per `SessionRuntime` (SQLite
   connections are cheap; avoids a global Mutex). Saving stays on the
   existing `save_to_db` path, now per runtime.
9. **Permission/question prompts**: each runtime owns its prompter pair;
   dialogs only ever show for the active session. Background runtime
   needing approval gets `waiting_permission` state + dashboard `!`.

### CLI: parallel fan-out (headless)

7. `whycodes generate` accepts repeated prompts or `--prompt-file` lines plus
   `-j/--jobs <N>`: runs N prompts concurrently via
   `Agent::spawn_parallel`-style Semaphore, each with its own `Session`
   (persisted), printing one result envelope per line in `--format json` /
   interleaved-tagged NDJSON in `stream-json`.

## Steps

- [x] S1: Extract `SessionRuntime` from `run.rs` locals; single-runtime
      behaviour unchanged. (done 2026-08-08, `9345bbb`)
- [x] S2: `Vec<SessionRuntime>` + active index + MRU; per-session state
      machine; drain-all event loop; lossless view-state switch; `Ctrl+N`
      new session, `Ctrl+PageUp/Down` order cycle, `Ctrl+Tab` MRU.
      (done 2026-08-08, `43f4e6a`)
- [x] S3: Dashboard overlay (grouped Needs-input → Working → Idle; peek,
      attach/detach, `Ctrl+O`); `/sessions` picker gains a "live" section;
      session close (persist + drop runtime, cap 8 with clear error).
      (done 2026-08-08, `43f4e6a` — dashboard + cap; close lands with the
      picker live-section follow-up)
- [x] S4: Per-runtime `Database` + permission/question prompters;
      `waiting_permission` badge for background approvals. (done 2026-08-08)
- [x] S5: CLI parallel fan-out for `generate` (`-j`). Failure mode: each
      prompt gets its own `result` envelope with `is_error` (per-prompt
      failures never abort siblings); process exit code is non-zero if any
      prompt failed; partial envelopes are always printed for completed
      prompts. (done 2026-08-08 — verified: two envelopes, two session_ids)
- [x] S6: Docs — FEATURES.md row to ✅, README keybindings/commands,
      status.md entry. (done 2026-08-08)

## Acceptance criteria

- Two sessions in one TUI: start a long turn in session A, `Ctrl+Tab` to
  session B, chat normally; A keeps working in background, its output is
  intact when switching back (transcript, scroll, draft all preserved).
- Permission prompt raised by a background session does not steal the
  active session's dialog; dashboard shows `!` on that session instead.
- Restarting whycodes: both sessions are persisted and resumable via
  `--resume` / `/sessions`.
- `whycodes generate "a" "b" -j 2 --format json` prints two envelopes, two
  distinct `session_id`s.
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D
  warnings`, `cargo test --workspace` all green.

## Risks / notes

- Read docs/knowhow.md before touching the event loop (mouse/event-loop
  return-value pitfalls, /dev/tty, silent exits).
- The `TurnOutcome` move-out/move-back pattern must stay per runtime; never
  share an `Agent` between runtimes (it carries per-session state:
  `cwd_override`, `activated_tools`, `subagent_usage_pending`).
- Memory: each runtime holds a full message history; cap live sessions
  (suggest 8) with a clear error.
- Competitive UX research (2026-08-08, librarian): Claude Code Agent View =
  dashboard reference; OpenCode tabs stalled; splits delegated to tmux.
  Tab bar rejected as primary surface.
