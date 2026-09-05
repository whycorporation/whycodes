# WhyCodes know-how

Living notes for bugs that are easy to reintroduce — especially TUI, terminal I/O, and silent exits. **Read this before changing the event loop, mouse handling, or terminal setup.**

When you fix a non-obvious bug: **append an entry** (newest first under [Log](#log)). Keep each entry short: symptom → root cause → fix → prevention.

---

## Quick diagnosis

| Symptom | First check |
|--------|-------------|
| TUI opens and dies immediately | `tail -40 ~/.local/share/whycodes/logs/unified.jsonl` |
| Panic? | `ls ~/.local/share/whycodes/crash/` (empty ⇒ usually not a panic) |
| Silent clean exit | Look for `tui.exit` / `tui.loop_error` / `main.exit_error` in JSONL |
| No TUI, plain mode | `stdin_tty` / `stdout_tty` / `/dev/tty` in `tui.starting` |
| CI `Budgets` / `Check & Lint` red | Rule 8 — run the three `scripts/check_*.py` + `clippy -D warnings` locally |

Lifecycle events written to **`~/.local/share/whycodes/logs/unified.jsonl`** (always-on):

| `msg` | Meaning |
|-------|---------|
| `logging.init` | Process started |
| `tui.starting` | About to open alt-screen |
| `tui.ready` | Raw mode + terminal created (`term_w` / `term_h`) |
| `tui.first_frame` | First successful draw (`w` / `h`) |
| `tui.context_window_applied` | Live `/v1/models` window applied |
| `tui.exit` | Loop left intentionally (`reason`) |
| `tui.loop_error` | Draw / poll / read failed |
| `tui.stopped` | Cleanup finished (`ok`) |
| `main.exit_error` | Top-level error (also printed once to stderr) |
| `llm.stream_chunk` | Mid-stream body decode failed (`error decoding response body` + source chain) |
| `turn.stream_error` | Agent loop received a stream `Err` (provider/model + message) |
| `turn.error` | TUI/CLI turn failed (display string) |

```bash
# After a bad run:
tail -40 ~/.local/share/whycodes/logs/unified.jsonl
```

---

## Hard rules (do not break)

### 1. `handle_event` return value = keep running?

```text
false  →  quit the whole TUI  (run.rs breaks the loop)
true   →  continue
```

**Never** use the return value for “needs repaint” / “hover unchanged” / “ignore this event”.

| Wrong | Right |
|-------|--------|
| `MouseEventKind::Moved => return hover_changed` | Always `return true` for move (keep running ≠ needs paint) |
| Move updates `mouse_pos` but never `mark_dirty` | On enter/leave hover chrome, call `app.mark_dirty()` so gated paint runs |
| `return false` meaning “skip rest of match” | Use `return true` or restructure control flow |

**File:** `crates/tui/src/input.rs` — `handle_event` / `handle_mouse`  
**Loop:** `crates/tui/src/run.rs` — `if !input::handle_event(...) { break; }` (paint only if `needs_redraw`)

### 2. TUI draw target when stdout is not a TTY

Hosts (IDE, wrappers) often report `stdout_tty=false` while still having a controlling terminal.

- Prefer **`/dev/tty`** for alt-screen + draws (`open_tui_writer` in `run.rs`).
- Fall back to stdout only if it is a TTY.
- Do not require `stdout.is_terminal()` alone to enter TUI mode (`tui_available()`).

### 3. SIGPIPE and closed stdout

Any write to a broken stdout pipe can **kill the process without a Rust panic** (no crash report).

- `ignore_sigpipe()` runs at process start (`crates/cli/src/main.rs`).
- Do not remove it without an equivalent safeguard.

### 4. Terminal size 0×0

Some PTYs report `TIOCGWINSZ` as 0×0. Drawing a zero-area buffer is useless and confuses diagnosis.

- After `Terminal::new`, if `term_size()` is 0 in either dimension, **`terminal.resize(80×24)`** and log `tui.size_fallback`.
- Do **not** call `supports_keyboard_enhancement()` when size is 0×0 or `WHYCODES_BENCH` is set — the CSI query times out at ~2 s and becomes the entire first-frame number.

### 5. `/v1/models` context window

- Config-driven only: `base_url` / `api_key` / headers from the active provider — **no hard-coded gateway hosts**.
- TUI must **not** store the full gateway catalog (thousands of models). Keep a single `api_context_window` for the active model.
- Failures are non-fatal; meter falls back to built-in / `session.max_context_tokens`.
- Opt-out: `WHYCODES_NO_MODEL_CATALOG=1`.

### 6. `max_tokens` vs `context_window`

| Field | Meaning |
|-------|---------|
| `ModelConfig.max_tokens` | Completion cap sent to the API |
| `ModelConfig.context_window` / API `context_length` | Full prompt+completion budget (meter, compact) |

Do not use rate-limit headers (`x-ratelimit-limit-tokens`) as context window — those are TPM quotas.

### 7. Build before “done”

See root `AGENTS.md`. After Rust edits: `cargo check` / `cargo build -p whycodes-cli` (and tests when logic changes).  
Users often run **`./target/release/whycodes`** — rebuild **release** when verifying TUI fixes they will run that way.

### 8. CI Budgets + Clippy — run locally, every Rust push

`cargo check` / tests going green is **not** enough. The `Budgets` job is a sequential Python gate that does **not** compile; `Check & Lint` is `fmt --check` + `clippy --workspace --all-targets -- -D warnings`. Feature commits keep landing red because the author never ran either.

**Before push, after any `.rs` / `Cargo.toml` change:**

```bash
python scripts/check_panic_budget.py
python scripts/check_swallowed_error_budget.py
python scripts/check_dependency_boundaries.py
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

The three budget scripts finish in seconds. They are the entire `Budgets` job. Run **all three** — the CI steps are fail-fast, so a swallowed-error fail **hides** the next boundary fail (2026-08-13: two rounds).

**Swallowed-error scanner** (`scripts/check_swallowed_error_budget.py`) counts these in non-test `.rs`:

| Pattern | Label |
|---------|--------|
| `let _ = foo(...)` | discarded result |
| `Err(_) =>` | unnamed Err arm |
| `.ok();` | Result dropped via `.ok()` (also matches `return x.ok();`) |

Prefer **handling** over raising the crate’s number in `scripts/swallowed_error_budget.json`:

- Name the error (`Err(e)` / `Err(elapsed)`), log it, or convert with `if let Ok`.
- Channel send: `if let Err(e) = tx.send(...) { tracing::debug!(...) }` — not `let _ =`.
- Mutex poison: `let Ok(g) = lock else { warn!(...); return; }`.
- `return n.try_from().ok();` is a **false positive** (returns `Option`, does not drop). Rewrite to `if let Ok(n) = … { return Some(n); }` so the scanner stays quiet.

Only bump a budget in the **same commit**, and say why. If the count is *below* budget, **lower** the JSON to lock the improvement in (the script prints this).

**Dependency boundaries** (`scripts/dependency_boundaries.json`): any new `whycodes-*` line in a crate’s `Cargo.toml` is a new **edge**. Add it to that crate’s list in the same commit, with a reason. New crate ⇒ entry in **all three** JSONs (see 2026-08-09 log).

**Clippy:** `-D warnings` means style lints fail CI. Common one: `if let Some(x) = … { v } else { return None; }` → `let x = …?;`.

---

## Log

### 2026-09-05 — Agent `bash`/`shell` tools spawn WSL2 on this Windows host

**Symptom:** `bash`, `shell`, and `schedule` fail immediately with
`HCS_E_HYPERV_NOT_INSTALLED` / “WSL2 başlatılamıyor”. `git_status` /
`git_commit` still work.

**Root cause:** Those tools go through a Linux sandbox path that starts
WSL2. This machine has no Hyper-V/WSL. Native `git.exe` wrappers do not.

**Fix / workaround:** Do not use WSL. Prefer `git_*` tools and Git for
Windows (`git.exe`). Do not ask the user to install WSL.

**Prevention:** Windows hosts without WSL must keep a native command
path (cmd/PowerShell/`git.exe`), not `wsl.exe`.

### 2026-09-05 — TUI resume tests saw a sibling session DB

**Symptom:** `run::tests::apply_resume_found_missing_and_latest` failed
asserting a "No saved sessions" toast (784 passed / 1 failed under
`cargo test -p whycodes-tui --lib`).

**JSONL / crash:** none (unit test).

**Root cause:** `with_session_db` keeps a process-lifetime SQLite handle.
`isolate_home_fresh()` pointed `WHYCODES_HOME` at an empty temp dir, but
the cached connection still listed sessions from a sibling persist
(`persist_session_best_effort` / shared test home). `WHYCODES_HOME`
mutations in `persist.rs` / `session_runtime` also skipped the TUI env
lock.

**Fix:** `reset_session_db_cache()` after every test home switch; crate
`ENV_LOCK` covers all `WHYCODES_HOME` writes.

**Prevention:** Tests that persist then load, or that assert an empty
store, must hold `ENV_LOCK` for the whole sequence (`let _home =
isolate_home()`). Do not `set_var` the home without the lock. `test_runtime()`
must not re-lock when the home is already set.

### 2026-09-04 — `kill(u32::MAX)` logged the desktop session out

**Symptom:** `cargo test -p whycodes-cli` (lockfile / serve takeover tests)
dropped the Plasma session to the login screen while the suite was running.

**Root cause:** `signal_term(u32::MAX)` / `signal_kill(u32::MAX)` cast the pid
with `as i32` → `-1`. `kill(-1, SIGTERM|SIGKILL)` broadcasts to every process
the user can signal (except init). `kill(0, …)` is the process group.

**Fix:** `unix_kill_pid` only forwards pids that fit in a **positive** `i32`.
0 / overflow → `PidProbe::Dead` / `ErrorKind::NotFound`. Tests use
`unused_pid()` (2e9), never `u32::MAX`.

**Prevention:** `invalid_unix_pids_are_not_broadcast`. Never `pid as i32`
into `kill(2)`.

### 2026-09-04 — Default TUI `auto` never opened the question panel

**Symptom:** Interactive TUI in default `auto` silently picked option 1. Discord
`need_input` never fired for questionnaires. Serve timed out at 5 min vs TUI
30. Subagents blocked on stdin under the alt-screen.

**Fix:** Interactive TUI defaults to `important` (config omitted). Auto-picks
stamp `(auto-picked; approval_mode=auto — not a user choice)`. Channel/serve
prompters call `spawn_need_input_wait`. Serve uses `[tools.question]` timeout
and includes `preview`. `POST /v1/.../question` validates answers (400).
Subagents return a tool error instead of stdin.

**Prevention:** Do not route TUI `question` through `AutoAnswerPrompter`. Do
not call `ToolExecutor::execute` for `question` in subagents.

### 2026-09-03 — Windows TUI painted grayscale after RGB quantize


**Symptom:** On Windows PowerShell / Windows Terminal the whole TUI went
black-and-white. Agent-tinted prompt borders (build green, plan purple, …)
collapsed onto gray.

**JSONL / crash:** none (`tui.ready` `color_mode` is `"16"`).

**Root cause:** `#50` quantized RGB when the host is not truecolor. Windows
Terminal and classic conhost leave `TERM` / `COLORTERM` empty, so detection
fell through to Ansi16. Pastel theme tokens are Euclidean-closer to silver
(index 7) than to green/cyan, so every accent became gray.

**Fix:** Treat `WT_SESSION`, `ConEmuANSI=ON`, and `cfg!(windows)` as
truecolor (Win10+ VT). `WHYCODES_COLOR` still overrides. 16-colour quantize
skips gray slots for chromatic RGB so a forced `16` mode keeps agent hues.

**Prevention:** `cargo test -p whycodes-tui windows_empty_term_is_truecolor
ansi16_keeps_chroma_off_gray`. Manual: Windows Terminal + PowerShell in
`docs/tui-term-matrix.md`.

### 2026-09-03 — Serve lock lives under `$CWD/.whycodes/`, not data_dir

**Symptom:** A second `whycodes serve` printed `Address already in use` with no
pid or URL. Tests that bound a port also wrote `.whycodes/serve.lock` into the
repo working tree.

**Cause:** The lock is project-local (so `connect` in that tree can name the
holder). `cmd_serve` uses `current_dir()`. Completions and `--version` must not
create that directory.

**Fix:** atomic JSON lock (`pid`, `port`, `started_at`); stale = dead PID /
24h / clock skew; `--no-takeover` and CI skip the TTY prompt. Tests `chdir`
into a temp dir under `CWD_LOCK`.

**Prevention:** Do not bind `serve` from unit tests without `IsolatedCwd`. Do
not treat `EPERM` on `kill(pid, 0)` as stale.

### 2026-09-03 — Auto mode must not ask while todos/tasks are open

**Symptom:** In `auto` approval, the model still called `question` (or the
first option was auto-picked) while the sticky todo list / background jobs
were unfinished, so the user was interrupted mid-work. Failures also stopped
after one attempt.

**Root cause:** `execute_with_permission` auto-answered every `question` and
did not retry transient tool errors.

**Fix:** While session todos `has_open` or `background.running_count() > 0`,
auto mode refuses `question` with a keep-working hint. Transient tool errors
retry up to `AUTO_TOOL_RETRY_LIMIT` (2 extra) times. Policy denials, doom
loop, task/swarm/question are not retried.

**Prevention:** Tests `auto_refuses_question_while_todos_open` /
`auto_retries_transient_tool_failure`. Do not auto-answer `question` as a
substitute for finishing the list.


### 2026-09-02 — Plain REPL treated stdin EOF as an empty line

**Symptom:** `cmd_run --plain` with piped/scripted stdin never exits: after the
last line, `read_line` returns `Ok(0)` (EOF), `trim()` is empty, the loop
`continue`s and spins. Unit tests that needed slash-command coverage could not
drive the REPL without hanging.

**Root cause:** Only `read_line` `Err` broke the loop. POSIX EOF is `Ok(0)`
with an empty buffer.

**Fix:** `read_repl_line` (stdin, or a test queue via `WHYCODES_TEST_LLM` +
`install_test_repl_lines`) treats `Ok(0)` and `Err` as exit. Tests inject a
repeating `ScriptedProvider` so turns never hit the network.

**Prevention:** Any new interactive `read_line` loop must treat `Ok(0)` as
EOF. Drive `--plain` slash commands through `install_test_repl_lines`, not
by hoping stdin is a TTY.

### 2026-09-02 — Coverage skips watcher test, index floor drops 2 lines

**Symptom:** `Coverage (line floor)` fails `whycodes-index: 796/798 lines 99.7%`
after llvm-cov is found. `lib.rs` 590/10 missed, 367/2 regions.

**Root cause:** The coverage job `--skip tests::watcher_picks_up_changes`
(notify flake). That was the only path hitting `scanner_main`'s
`RecvAct::Batch` + `pending.extend` / `apply_changes` apply. Direct
`apply_changes` tests do not execute the loop arms.

**Fix:** `scanner_applies_batched_command` sends `Command::Batch` on the
channel with `watch: false` (no notify). Same 100% floor, no flake.

**Prevention:** Do not rely on skipped flake tests for a 100% crate floor.
Channel-inject the command instead of waiting on inotify.

---

### 2026-09-02 — Coverage wrapper: rustup llvm-cov is not on PATH

**Symptom:** CI `Coverage (line floor)` fails immediately with
`error: llvm-cov not found` after `scripts/coverage.sh` replaced the
inline `cargo llvm-cov` step. `taiki-e/install-action` had just installed
`cargo-llvm-cov`.

**Root cause:** rustup's `llvm-tools-preview` puts `llvm-cov` /
`llvm-profdata` under `$(rustc --print sysroot)/lib/rustlib/<host>/bin`,
not on `PATH`. `cargo llvm-cov` locates them; the wrapper's
`command -v llvm-cov` does not.

**Fix:** Prepend the rustlib bin dir to `PATH` when `llvm-cov` is
executable there. Distro toolchains still use `LLVM_COV` / PATH.

**Prevention:** Do not treat `command -v llvm-cov` as the rustup check.
`scripts/test_coverage.sh` asserts the sysroot prepend stays in the
wrapper.

---

### 2026-09-02 — CI Build (linux) fails without bundled SQLite

**Symptom:** `Build (linux)` (and Coverage) fail ~5m in with
`rust-lld: error: unable to find library -lsqlite3` after
`perf(build): make rusqlite bundled opt-in`.

**Root cause:** Workspace `rusqlite` no longer enables `bundled` by default
(dev builds use system sqlite via pkg-config). The self-hosted runner has no
`libsqlite3-dev`, and CI/release still ran `cargo build --release -p
whycodes-cli` / `cargo test --workspace` without `--features`.

**Fix:** CI clippy/test/coverage/build and `release.yml` pass
`whycodes-storage/bundled` or `whycodes-cli/bundled-sqlite`. Homebrew `--HEAD`
does the same.

**Prevention:** Machines without system sqlite must pass the feature. Do not
re-enable `bundled` on the workspace dep — that brings back the 43s C compile
on every cold check.

---

### 2026-09-02 — Leftover idle 0.3/s after first-frame hydrate

**Symptom:** Empty-project harness `WHYCODES_BENCH_DURATION_MS=3000` still
**0.32 draws/s** (DRAWS=2: first paint + exactly one more in 3s).

**Root cause:** After first paint, hydrate still scheduled a second frame:
`maybe_session_auto_index` toasts on `Some(0)` (empty tempdir indexes 0
chunks) so `first_frame_hydrate_needs_paint` is true; `replace_todos` /
other hydrate work can leave `needs_redraw` even when the helper is
false; `spawn_update_check` can deliver `update_rx` within 3s.

**Fix:** Skip GitHub update when `WHYCODES_BENCH` is set. Do not toast
on 0 chunks. `settle_first_frame_hydrate` forces `needs_redraw=false`
and `pending_full_clears=0` when chrome is unchanged and not animating.

**Prevention:** Never `current_thread` for TUI. Tests:
`auto_update_only_interactive_text_sessions` (bench gate),
`auto_index_zero_chunks_does_not_toast`, `first_frame_hydrate_settle_*`.

---

### 2026-09-02 — First-frame hydrate must not idle-repaint empty home

**Symptom:** 3s first-frame harness idle 0.3/s after multi-thread restore
(was 0.0/s on `current_thread` because hydrate work never ran during
`event::poll`).

**Root cause:** After first paint, hydrate filled session recents / status /
sidebar / toasts. Unconditional `mark_dirty` if toasts nonempty; session
list fill had no dirty. Empty home still often painted twice because
status rewrite or toast.

**Fix (4155fc6):** `first_frame_hydrate_needs_paint` — dirty only if
sessions len, `status_message`, visible sidebar lists, or toasts changed.
MCP / index / plugins still run.

**Prevention:** Never `current_thread` for TUI. Regression tests
`first_frame_hydrate_*` in `crates/tui/src/run/tests.rs`.

---

### 2026-09-02 — TUI multi-thread pool capped at 2

**Symptom:** After `de15f1a` restored multi-thread for TUI (poll blocks;
`current_thread` starves spawned turns), TTFF 11.1→12.5 ms and 1-session
PSS 8.4→11.8 MB. `runtime_for` used `Builder::new_multi_thread()` with
no `worker_threads()` — default ≈ nproc.

**Root cause:** Interactive TUI only needs a couple of workers (turn HTTP
+ hydrate) while `event::poll` occupies one thread. Spawning nproc
workers is extra TTFF/RSS.

**Fix:** `TUI_WORKER_THREADS = 2` on the multi-thread path for TUI / `run`
and other non-generate/serve commands. Generate / Serve keep the default
nproc pool. Never `current_thread` for TUI.

**Prevention:** Keep `command_needs_multi_thread` true for `None` and
`Commands::Run`. Regression: `runtime_choice_per_command` /
`runtime_for_builds_the_selected_runtime_flavor`.

---

### 2026-09-02 — Prompt paste drops ASCII `i` (#56)

**Symptom:** Pasting `iyi` / `istanbul` into the TUI prompt drops every `i`
(`y`, `stanbul`). Dotless `ı` types and pastes. Prompt-focused typing of
`hi` is fine.

**Root cause:** Scrollback / todos bound bare `Char('i')` to
`Action::FocusPrompt` (vim insert). Hosts without bracketed paste deliver
a short paste as a key flood; `coalesce_unbracketed_paste` only folds 2+
lines or ≥160 chars. Each `i` then focuses the prompt and is not inserted.
Later letters auto-focus and type. `ı` is a different code point.

**Fix:** Unbind printable `i`. Return to the prompt with Tab / Space /
Enter / Esc / Backspace. Unmapped letters still auto-focus the prompt
*and* insert.

**Prevention:** Do not bind a letter that appears in real language as a
focus chord. Regression: `turkish_i_and_dotless_i_type_and_paste`;
keymap `Char('i')` is `None` in scrollback/todos.

---

### 2026-09-02 — TUI `current_thread` starves spawned turns

**Symptom:** Home paints. Typing `selam` + Enter shows the user bubble and
"Working…" (or equivalent busy chrome) then nothing for minutes. No
`turn.step` / `llm.stream_chunk` in `unified.jsonl`. Esc eventually writes
`session.persist` with `reason: force_cancelled`.

**Root cause:** #49 put bare TUI / `run` on `tokio` `current_thread` to skip
worker-pool spawn before first paint. The event loop then blocks the only
runtime thread in `crossterm::event::poll`. `tokio::spawn` turn / stream /
catalog tasks are queued and never polled until the next input or timeout.
A turn that needs HTTP therefore never starts.

**Fix:** `command_needs_multi_thread` is true for `None` and `Commands::Run`.
TTFF still uses paint-then-hydrate; do not "save" pool spawn by starving the
turn.

**Prevention:** Never put the TUI on `current_thread` while `event::poll` is
synchronous. Regression: `runtime_choice_per_command` /
`runtime_for_builds_the_selected_runtime_flavor`.

---

### 2026-09-01 — Split `agent.rs` by responsibility (#48)

**Symptom:** `crates/agent/src/agent.rs` was ~4.4k lines: turn loop, permission
gates, compaction, and swarm/task dispatch in one file.

**Root cause:** Incremental features landed in the facade instead of sibling
modules (`tool_policy` already existed for formatting).

**Fix:** `agent.rs` → `agent/{mod,turn,gate,dispatch,spawn,compact}.rs`.
`Agent` remains the public facade; `impl Agent` is split across modules.
No behaviour change. `TuiApp` still ~120 fields (P1 remainder).

**Prevention:** New turn-loop / gate / swarm code goes in the matching file.
Do not grow `agent/mod.rs` with execution paths.

---

### 2026-09-01 — Tool schema cache + `ErrorKind` on `core::Error` (#48)

**Symptom:** Each agent LLM step rebuilt every `Tool::definition()` JSON schema.
Retry/TUI classified LLM failures by parsing `Error` display strings.

**Root cause:** `ToolExecutor::get_definitions*` allocated a fresh `Vec<ToolDefinition>`
with no memo. `Error::Llm` / `Error::Http` were `String`.

**Fix:** Cache `Arc<[ToolDefinition]>` keyed by a stable permission fingerprint +
profile + extra names; invalidate on `register`. `LlmRequest.tools` is `Arc<[ToolDefinition]>`.
`TransportError { kind, message }` on LLM/HTTP; `classify()` trusts structured kind.

**Prevention:** `cargo test -p whycodes-tools definitions_cache_reuses_arc_until_register`;
`cargo test -p whycodes-llm classify_prefers_structured_kind_over_display_string`.
Swallow ratchet: tui 39 / cli 26 / agent 18.

---

### 2026-09-01 — Ctrl+V pastes a screenshot from the OS clipboard

**Symptom:** Copying a screenshot (or a browser image) and pressing Ctrl+V in
the prompt did nothing. Drag-drop / path paste already attached files.

**Root cause:** `Event::Paste` is text. Terminals never deliver PNG bytes.
Clipboard I/O was write-only text (`OSC 52` / `wl-copy` / `xclip` / `pbcopy`).
Ctrl+V was an unmapped chord (swallowed so it would not type `v`).

**Fix:** Bind Ctrl+V → `Action::PasteClipboard`. `clipboard_image` reads a
bitmap via `wl-paste` / `xclip` / macOS pasteboard / PowerShell, sniffs magic
bytes, stashes under `<data_dir>/clipboard-images/`, then `attach_image`.
Text stays on bracketed `Event::Paste` so hosts that intercept Ctrl+V do not
double-insert. Empty clipboard is silent. Idle TUI stays 0 draws/s (read is
on the key event).

**Prevention:** `cargo test -p whycodes-tui` — sniff, stash, keymap Ctrl+V,
input stub (no live compositor). Manual: `docs/tui-term-matrix.md` check 8b.

---

### 2026-09-01 — Apple Terminal.app RGB + DIM leak to white / build-green

**Symptom:** On macOS Terminal.app, modal chrome (help, pickers, question)
paints white instead of the theme RGB. Thinking rails / muted transcript
pick up the build-agent green (`palette.success`) instead of
`palette.thinking` / `palette.dim`. iTerm2 / Ghostty / Alacritty look fine.

**Root cause:** Terminal.app does not honour `38;2` / `48;2`. A failed
truecolor SGR leaves the cell on the profile default (white). `Clear` on
the modal rect resets fg/bg, so the next RGB write is dropped. `Modifier::DIM`
(`2m`) on a truecolor fg is mapped onto ANSI colour 2 (green) — the same
slot as the build agent.

**Fix:** Detect colour capability (`COLORTERM`, `TERM_PROGRAM=Apple_Terminal`,
`TERM`, `WHYCODES_COLOR`). Quantize palette RGB to xterm-256 / 16 *before*
paint and wrap `CrosstermBackend` so a stray `Color::Rgb` never reaches the
wire. Replace modal `Clear` with `fill_blank(palette.bg)`. Paint thinking
body with `palette.dim` and no SGR DIM.

**Prevention:** `cargo test -p whycodes-tui` covers the quantizer, modal fill,
and thinking styles. Manual: `docs/tui-term-matrix.md` Apple Terminal.app
row. Override with `WHYCODES_COLOR=256`.

### 2026-08-31 — Prompt `:` must not steal into vim command mode

**Symptom:** Typing `:` (or `::`, a URL, `std::io`) in the TUI prompt
opens a separate command field (`: ` prefix). The chat draft is hidden;
Esc is required to leave.

**Root cause:** Prompt-focused `:` was bound to `Action::EnterCommand`,
which switched `AppMode::Command` and seeded `command.buffer` with `":"`.
The colon command set (`:q`, `:theme`, …) duplicated slash commands.

**Fix:** `:` is unmapped printable text. Sessions dashboard uses
`AppMode::Dialog` (it had borrowed `Command` as a stand-in). Help lists
`/theme` / `/exit` instead of `:theme` / `:q`.

**Prevention:** Do not bind a printable character as a global prompt
action. Regression: `colon_types_into_the_prompt_instead_of_opening_command_mode`.

### 2026-08-31 — Homebrew 6 tap trust blocks `brew install whycodes`

**Symptom:** `brew tap whycorporation/whycodes https://github.com/whycorporation/whycodes`
succeeds (clones the whole repo into
`Taps/whycorporation/homebrew-whycodes`), then `brew install whycodes`
fails with `Refusing to load formula … from untrusted tap`. Homebrew
prints `brew trust --formula whycorporation/whycodes/whycodes`.

**Root cause:** Homebrew 6.0.0 requires explicit trust for non-official
taps before evaluating formula Ruby. Short-name `brew install whycodes`
does not grant that trust. Fully-qualified
`brew install user/repo/formula` trusts only that item.

**Fix:** Document `brew install whycorporation/whycodes/whycodes` after
the custom-URL tap (this repo *is* the tap; there is no
`homebrew-whycodes` remote). Already-tapped machines:
`brew trust --formula whycorporation/whycodes/whycodes && brew install whycodes`.

**Prevention:** Keep README / guide / packaging / formula header /
landing brew snippet on the fully-qualified name. Do not tell people to
`brew trust whycorporation/whycodes` unless they accept every future
formula in the tap.

### 2026-08-30 — Prompt Backspace/Delete must not `terminal.clear()`

**Symptom:** Typing in the prompt is smooth; Backspace, Delete, Ctrl+U, and
word-kill flash the whole TUI (blank frame, then full repaint) on both the
home screen and in-session chat.

**Root cause:** `handle_input_action` called `request_full_clear(1)` on every
prompt shrink. That path exists for *paste echo*: hosts write glyphs onto the
PTY outside ratatui's diff, and the centered home gutters stay spaces in both
frames so the leftover cannot be erased without a hard clear. Ordinary
grapheme/word delete never produces that echo. `fill_blank` already paints
the home gutters, so a normal dirty redraw is enough.

**Fix:** Drop `request_full_clear` from Backspace / Delete / Ctrl+U / word-kill.
Keep it on `insert_paste_text`, submit, session switch, and
`event_needs_full_clear` (Paste / Resize / FocusGained).

**Prevention:** Do not treat "the prompt got shorter" as "the PTY is dirty".
Only force `terminal.clear()` when something wrote *outside* ratatui (paste
echo, resize, focus restore) or the layout jumps (submit / new session).

### 2026-08-30 — Session token usage: Codex double-count, cold cache, stale meter

**Symptom:** ChatGPT/Codex `/cost` and the context bar were high after cache hits.
Long sessions re-walked the whole transcript before every LLM step. After a
tool-using turn the meter stayed on the last billed *prompt*, not the next
prefill.

**Root cause:**
1. Codex mapped OpenAI-style `input_tokens_details.cached_tokens` (a **subset**
   of `input_tokens`) into additive `CacheUsage`. `Usage::total()` and
   `context_tokens_from_usage` then counted cache twice. Chat-completions
   already left those fields unset.
2. `truncate` / `prune` / `shake` called `token_cache.invalidate()`.
   `token_count()` is `&self` and does not rebuild; `push_msg` is a no-op
   while invalid. The agent then called `token_count()` twice per step.
3. Turn-end only `sync_context_estimate` when `turn_usage` was missing, so a
   successful usage event froze the meter on that step's prompt.

**Fix:** Codex emits `Usage` only (same as OpenAI-compat). Prune paths
`rebuild` the cache; the agent uses `token_count_cached()`. Turn Ok/cancel
always syncs the meter from the live transcript. Last-step usage still
updates the meter *during* the stream.

**Prevention:** OpenAI-family `cached_tokens` must never become Anthropic
`cache_read_input_tokens`. Mutating compact/prune must leave `token_cache`
valid. Do not skip transcript sync at turn end because billed usage exists.

### 2026-08-30 — Question popup Esc / `[✗]` left the agent hung (issue #41)

**Symptom:** `question` tool panel stayed open on Esc, or closed on mouse/`[✗]`
while the turn never continued.

**Root cause:** (1) Empty free-text focus (option-less questions, Other) ate
the first Esc without cancelling. (2) `pending_question_answers` /
`question_dismissed` were drained **before** `handle_event` and only inside
`if has_ev`, so a click never completed the oneshot until another event.
(3) Busy-turn Esc ran even with the overlay open and cancelled the wait
without popping the dialog. (4) `maybe_open_queued_dialog` keyed off
`WaitingForQuestion` with an empty stack, so a dismissed panel never
reopened.

**Fix:** Drain oneshot flags **after** `handle_event` and on idle ticks;
empty free-text Esc cancels immediately; overlay keys beat busy-cancel;
`begin_cancel` / `force_stop` close permission/question chrome; queue
opener keys off the actual dialog.

**Prevention:** Overlay keys (permission / question) must run before
busy-cancel. Oneshot flags written in `input.rs` must be flushed after
`handle_event` **and** every loop tick, not only when `has_ev`.

### 2026-08-30 — `should_auto_update` unit test fails on GitHub Actions

**Symptom:** `Test (linux)` red in ~1 min:
`cli::tests::auto_update_only_interactive_text_sessions` panics on
`assert!(should_auto_update(&cli(None), true))`.

**Root cause:** `should_auto_update` treats `CI` as a production opt-out
(headless/CI must not phone GitHub Releases). GitHub Actions sets
`CI=true`, so the "interactive session should check" assertion is
always false in CI even though the command/config logic is correct.

**Fix:** Gate env flags through `should_auto_update_with_env` and assert
those in the unit test. Do not `set_var`/`remove_var` `CI` in parallel
tests — it is process-global.

**Prevention:** Tests that encode "feature on" must not read `CI` /
`WHYCODES_NO_*` from the live process. Inject the flags.

### 2026-08-30 — `anthropic/` on a local proxy still demanded an API key

**Symptom:** Model picker `anthropic/…` showed “No API key for anthropic”
even when `[providers.anthropic] base_url = "http://127.0.0.1:…"` pointed
at a local LiteLLM / Ollama / reverse-proxy.

**Root cause:** Two stacked bugs:

1. `provider_requires_api_key` only skipped `ollama`. Built-in Anthropic
   (and OpenAI) always required a credential, so the TUI never sent the
   turn.
2. `AnthropicProvider` always posted to `https://api.anthropic.com/v1/messages`,
   ignoring config `base_url`. A local proxy never saw the request.

**Fix:** Local/loopback `base_url` skips the key gate for any provider.
Anthropic and OpenAI rebuild their request URL from config. Empty keys
omit `x-api-key` / `Authorization` instead of sending a blank header.

**Prevention:** Do not treat the model id prefix (`anthropic/`) as
“must talk to api.anthropic.com”. Honor `base_url`. Keep cloud hosts
key-gated.

### 2026-08-30 — Ollama chat hangs if URL/key/stream-stop are wrong

**Symptom:** Selecting Ollama (or posting to a host on a non-default port
such as `:4554`) never produced a reply. The TUI/CLI sat on "generating".

**Root cause:** Three stacked bugs:

1. `OllamaProvider` always posted to `http://localhost:11434/api/chat`.
   Config `base_url` / `api_base` and `OLLAMA_HOST` were ignored, so a
   daemon on `:4554` never saw the request.
2. TUI, CLI, and `whycodes serve` required an API key for every
   provider. Local Ollama has none, so the turn never left the gate.
3. The agent loop waits for `StreamEvent::MessageStop`. Ollama NDJSON
   without a trailing newline (or without `done`) dropped the last
   object and never emitted stop — the turn hung forever.

**Fix:** Rebuild the Ollama client from config/`OLLAMA_HOST`, skip the
API-key gate for `ollama`, flush leftover NDJSON, and emit
`MessageStop` on EOF.

**Prevention:** Do not hard-code `11434`. Do not treat Ollama as
OpenAI SSE. Keep `provider_requires_api_key("ollama") == false`.

### 2026-08-30 — Sticky todo overflow must scroll, not `+N more`

**Symptom:** Long session todo lists hid extra rows behind `… +N more`. Wheel
events always moved the transcript, so the hidden items were unreachable.

**Root cause:** `todos::render_panel` reserved a row for an overflow label
instead of a window. `coalesce_chat_wheels` folded *every* wheel into
`scroll_rows` before `handle_mouse`, so a hit-test in the mouse path never
saw the flick.

**Fix:** Item rows scroll in a `MAX_ITEMS` window with the shared solid
scrollbar. Wheel over the expanded panel (including coalesced flicks) calls
`scroll_todos`. Keyboard focus `FocusPane::Todos` (click the rows, or Tab
when overflowing) maps ↑/↓/j/k. Header click / `t` still folds.

**Prevention:** Keep todo-hit routing in `coalesce_chat_wheels`. Do not
reintroduce a `+N more` line.

### 2026-08-30 — Home-screen update is a confirm, never a silent replace

**Symptom:** Interactive start used to call `upgrade::run_quiet()` before
the TUI painted. A newer GitHub tag replaced the binary with no prompt.

**Root cause:** Auto-update was wired as a silent install so the next
process start would pick up the new binary. Users never saw a choice, and
a Homebrew prefix still needed a different message.

**Fix:** Background `check_latest` after first paint. Empty home screen
opens a confirm (`Update now?`) or a Homebrew alert. Accept quits the TUI
(`TuiExit::Upgrade`) and the CLI installs after the terminal is restored.
`--no-auto-update` / `CI` / `WHYCODES_NO_AUTO_UPDATE` still skip the check.

**Prevention:** Do not call `upgrade::run` from TUI boot. Keep the GitHub
fetch off the first-paint path.

### 2026-08-30 — Interactive auto-update must not run in CI or headless

**Symptom:** A pipeline that invokes `whycodes generate` or `--format json`
could replace the binary mid-job if GitHub had a newer tag.

**Root cause:** Self-update talks to GitHub Releases. That is fine for a
human TUI start, not for CI, ACP, or structured output.

**Fix:** `should_auto_update` is true only for a bare invoke / `run` with
text output, and never when `--no-auto-update`, `CI`, or
`WHYCODES_NO_AUTO_UPDATE` is set. Homebrew prefixes still refuse to
self-replace.

**Prevention:** Keep auto-update off the headless command match arm. The
TUI path must confirm before `upgrade::run`.

### 2026-08-30 — Tool-result cap footer is billed every turn; recap must strip it

**Symptom:** Capped `bash`/`read`/`grep` dumps keep a long
`[... N characters truncated for context management]` footer in the model
transcript. Re-running prune/shake then reports the *footer* as omitted payload
and the original `N` is lost.

**Root cause:** `cap_tool_text_to` (`crates/session/src/session.rs`) appends the
notice after taking `max_chars`, then treats the whole string as payload on the
next pass. The notice is paid on every later LLM request, not just once.

**Fix:** Footer is `\n[{n} chars truncated]` (cl100k: 7 tokens vs 12). Strip a
trailing current/legacy notice before measuring so recap is idempotent and
preserves the original omitted count.

**Prevention:** `image_payload_parsing_and_unicode_caps_cover_boundaries` covers
idempotent recap, tighter recap, and legacy rewrite. Do not lengthen the footer.

### 2026-08-29 — Windows TUI footer shows `\\?\C:\…` after canonicalize

**Symptom:** Bottom-left chrome paints `≡ main  \\?\C:\dev  0 / 200k`. Click-to-copy
pastes the same extended path.

**Root cause:** `app.project_dir` is `Path::canonicalize()` (`crates/tui/src/run.rs`).
On Windows that returns a Win32 verbatim path (`\\?\C:\…` / `\\?\UNC\…`). The
footer and copy handler used `Path::display()` as-is.

**Fix:** Keep the canonical `PathBuf` for FS work. Format for humans with
`whycodes_core::display_path`, which strips `\\?\` / `\\?\UNC\` for drive and
UNC paths and leaves device-namespace paths (`\\?\pipe\…`) alone.

**Prevention:** `paths_tests::display_path_strips_windows_verbatim_prefix` and
`footer_strips_windows_verbatim_cwd`. Do not `display()` a canonicalized
Windows path in chrome.

### 2026-08-28 — Homebrew formula is a binary tap, not a self-update target

**Symptom:** `brew install whycodes` against `v0.1.0` 404s (`whycodes-*.tar.gz`
does not exist). Completions generation writes `~/.local/share/whycodes` and
fails in Homebrew's sandbox. `whycodes upgrade` would overwrite Cellar
binaries and break `brew doctor`.

**Root cause:** The first tagged release shipped `whycode-<triple>.tar.gz`
with a `whycode` binary. `Formula/whycodes.rb` was regenerated for the
post-rebrand `whycodes-*` names. `whycodes completions` always initialized
logging. Self-update always replaced `current_exe()`.

**Fix:** Formula updater falls back to `whycode-*` + `bin.install "whycode"
=> "whycodes"`. Completions short-circuit before Tokio/logging. Upgrade
refuses Homebrew prefixes (`Cellar`, `/opt/homebrew`, Linuxbrew). Formula
installs completions, `livecheck`, and a `brew upgrade` caveat. macOS
release builds set `MACOSX_DEPLOYMENT_TARGET`.

**Prevention:** `scripts/test_update_homebrew_formula.sh` covers current and
legacy SHA256SUMS. `upgrade_helpers::homebrew_prefix_is_not_self_updated`
locks the Cellar/prefix check.

### 2026-08-28 — Ctrl+G is a sticky Tasks panel, not the sidebar

**Symptom:** Ctrl+G opened the Agents sidebar (block borders, “No subagents”,
a `Ctrl+G` hint) while Todos sat as a Grok header + progress track under the
status bar. Hovering off the effort chip left it underlined. Opening a child
transcript left the native prompt caret blinking through the overlay.

**Root cause:** `ToggleTasksPane` flipped `sidebar.visible` / `SidebarTab::Agents`.
`update_chrome_hover` forgot `effort_hit` on mouse-leave. `prompt_owns_caret`
only checked `modal_is_open()`, and the framed child is not a dialog.

**Fix:** Sticky Tasks panel reuses the Todos chrome (▸/▾ header, `done/total
pct%` track, status-bar background, chat-column indent). Ctrl+G / header click
folds it; item click inspects a subagent. Empty list still falls back to the
Agents tab. Hover leave now clears `effort_hit`. Caret hides while
`open_subagent` is set.

**Prevention:** `ui::subagents` tests cover empty/collapse/overflow/indent.
`tasks_panel_click_toggles_collapse` and the Ctrl+G input test assert the
sidebar stays closed when work exists.

### 2026-08-27 — bash timeout must kill the process group; writes are atomic

**Symptom:** `bash timeout=N` returned “timed out” but `sleep 999` / hung
`cargo test` kept running (and held a Tokio blocking thread). `edit`/`write`
used `std::fs::write`, so a crash could leave a half-written file. `bg` was
deferred while `bash background=true` stayed in Core.

**Root cause:** Timeout wrapped `spawn_blocking` from the outside;
`Command::output()` is not cancelled when the future is dropped. Writes were
in-place. Core shrank `bg` independently of background shell.

**Fix:** `run_timeout` lives inside the spawn, `setpgid` + `kill(-pid)`,
atomic temp+rename for edit/write/apply_patch, `bg` back in Core. Grep
searches files with rayon; index visit no longer clones the whole store.

**Prevention:** `run_timeout_kills_sleep` in sandbox tests. Atomic helper
has its own unit test.

### 2026-08-27 — grep uses in-process ripgrep; Core is ~12 names; edit is whitespace-tolerant

**Symptom:** `grep` walked files with the `regex` crate (no SIMD/mmap engine).
Core advertised ~20 tools (`apply_patch`, `swarm`, `schedule`, `memory`, …)
and blew the TTFT budget. `edit` failed when the model’s `old_string` only
differed in indent or extra spaces.

**Root cause:** Content search never used the ripgrep crates already in the
workspace. Core grew by adding every “useful” tool instead of deferring via
`tool_search`. Exact substring replace is brittle against LLM copy-paste.

**Fix:** `grep` uses `grep_searcher` + `grep_regex`. Core is the coding loop
plus `tool_search` (≤13 entries counting the `todo` alias). `edit` falls back
to a unique token-sequence match (whitespace between tokens may differ; typos
and glued tokens do not).

**Prevention:** `core_defers_specialized_tools` ratchets the list. Grep tests
cover context / binary / gitignore. Edit tests cover indent mismatch and
uniqueness.

### 2026-08-27 — file-tool cold walk must use gitignore; SERIAL_TOOLS names must match `Tool::name()`

**Symptom:** `grep`/`glob`/`list` advertised `.gitignore` respect, but the
cold `walk_files` path only pruned `SKIP_DIRS` / hidden dirs. Concurrent
`todowrite` calls could race because `SERIAL_TOOLS` listed `todo_write`.

**Root cause:** The workspace index already walked with `ignore`; file tools
fell back to a hand-rolled walker. Parallel-safety used a snake_case typo
that never matched the registered name (`todowrite`). `glob`/`list`/`read`/
`edit`/`write`/`git_*` also ran `std::fs` / `Command::output` on Tokio
workers. `apply_patch` shelled out to `patch(1)` (missing on Windows, PID
temp-file races).

**Fix:** Cold walks use `ignore::WalkBuilder` (same engine as the index).
`SERIAL_TOOLS` uses `todowrite`. FS/git tools go through `spawn_blocking`.
`apply_patch` applies unified diffs in-process.

**Prevention:** Keep a gitignore assertion on the cold walk. Assert serial
names against real `Tool::name()` registrations, not snake_case guesses.

### 2026-08-27 — prompt caret is a native blinking bar, not a software blink

**Symptom:** Wanted an insert-style `|` that blinks inside the prompt. A
cell-flip blink would force animation cadence (~25 fps) and break idle 0
draws/s.

**Root cause:** The emulator already blinks its own caret (DECSCUSR).
`frame.set_cursor_position` only places it; `SetCursorStyle::BlinkingBar`
(`CSI 5 q`) is a one-shot at TUI start. Software blink is a redraw loop.

**Fix:** Enable blinking bar after alt-screen; restore `DefaultUserShape`
(`CSI 0 q`) on exit / panic. Hide the caret when scrollback or a modal owns
keys (`prompt_owns_caret`). Do not treat the caret as `is_animating`.

**Prevention:** Assert caret placement + hide in `prompt.rs`. Assert restore
CSI in `restore_terminal_resets_cursor_style_to_user_default`. Never poll at
`REDRAW_ANIMATE` just to blink a glyph.

### 2026-08-26 — keyboard-enhancement CSI query stalled first frame ~2 s

**Symptom:** `bench_first_frame.py` reported ~2024 ms in-proc TTFF. Real
terminals were ~15–20 ms `tui.starting` → `tui.first_frame`. README could not
quote a harness first-frame number.

**Root cause:** `supports_keyboard_enhancement()` writes a CSI query and waits
crossterm’s ~2 s timeout. Dumb / 0×0 PTYs never answer. Separately,
`record_draw()` ran *after* blocking MCP connect + auto-index, so `--idle-ms 0`
could not exit at the first paint.

**Fix:** Skip the query when `WHYCODES_BENCH` is set or `TIOCGWINSZ` is 0×0
(`should_query_keyboard_enhancement`). Record the draw immediately after
`terminal.draw`, then stop the bench loop before MCP/index. Harness PTY is
sized 80×24 in the child before exec. Identical `replace_todos` no longer
`mark_dirty`s (empty boot reload was a one-shot extra paint, not an idle
lock).

**Prevention:** Never call `supports_keyboard_enhancement` on a 0×0 PTY.
`record_draw` / `should_stop` must run before any post-paint I/O. Assert the
skip in `keyboard_enhancement_query_skips_bench_and_zero_size`.

### 2026-08-26 — WhyCodes rebrand vs GitHub slug

**Symptom:** Mixing `Whycode` / `WhyCodes` / `whycode` / `whycodes`. Naive
`whycode` → `whycodes` replaces also turn `whycodes` into `whycodess`.

**Cause:** Product name is **WhyCodes** (`why.codes`). GitHub is
`whycorporation/whycodes`. Homebrew's class is the formula name camel-cased
(`whycodes` → `Whycodes`), not `WhyCodes`. A dummy Code Assist project id was
already the string `whycodes`.

**Fix:** User-facing copy and crates/binary/env are `WhyCodes` / `whycodes` /
`WHYCODES_*`. Clone, release, and raw.githubusercontent URLs are
`whycorporation/whycodes`. Homepage is `https://why.codes`. Installers and
Homebrew ship only `whycodes`. There is no `whycode` binary alias, env, or
project-local directory.

**Prevention:** Replace `whycode` only at identifier boundaries. Homebrew
class = formula filename, homepage = `https://why.codes`. Do not re-add a
`whycode` install alias, `WHYCODE_HOME`, or `.whycode/` fallback.

### 2026-08-26 — stream decode errors need JSONL at the chunk site

**Symptom:** TUI shows `LLM error: Stream: error decoding response body` with
no TLS/EOF/JSON cause. `unified.jsonl` only had TUI `turn.error` (display
string). One-shot CLI had no JSONL line at all.

**Cause:** provider `bytes_stream` `Err` branches did `format!("Stream: {e}")`.
`reqwest` Display is the outer message; `source()` holds the decode cause.
That `Err` was returned to the agent and never `logging::emit`'d.

**Fix:** `openai_compat::stream_chunk_error` walks the source chain, emits
`llm.stream_chunk`, and is used from every provider stream `Err` branch.
Agent also emits `turn.stream_error`; CLI `emit_turn_outcome` emits
`turn.error`.

**Prevention:** Never stringify a `reqwest`/`hyper` error with `{e}` alone
on the stream hot path. Log at the decode site, not only in the TUI.

### 2026-08-25 — tool summary `&raw[..56]` panics mid-UTF-8 (`ö`)

**Symptom:** TUI abort on a live turn:
`end byte index 56 is not a char boundary; it is inside 'ö' (bytes 55..57 of string)`
at `crates/tui/src/ui/chat.rs` (`tool_summary`). Crash file
`crash-20260825T163136.045.txt`.

**JSONL / crash:** panic in `main`; location is the JSON-dump fallback that
sliced the serialized tool input at a fixed **byte** index.

**Root cause:** unknown tools with no `command`/`path`/`query`/… string field
fall back to `input.to_string()`. If that dump is longer than 56 bytes, the
code did `&raw[..56]`. A 2-byte char (`ö`, CJK, emoji) straddling offset 56
is not a char boundary.

Same class in `crates/tui/src/widgets/message.rs`: `&content[..500]` on
tool-result preview.

**Fix:** `s.floor_char_boundary(n)` before the slice (`ellipsize_bytes` in
chat; inline in the message widget). Tests place `ö` on the old cut.

**Prevention:** Never `&s[..n]` / `String::truncate(n)` on user or JSON text
without `floor_char_boundary`. Byte length ≠ char length. Same lesson as
the 2026-08-23 memory-index clip.

Same class still lurked in MCP log `truncate` (`&s[..max]`),
`truncate_path` mid-cut (`&path[..third]`), and `/doctor` secret mask
(`&val[..4]`). All now floor/ceil to a char boundary.

### 2026-08-25 — Paste overflow returns on new-session home (grey boxes)

**Symptom:** A long paste paints outside the boxed prompt. Enter/submit cleans it up, but opening a new session (home) brings the leftover text back. Grey boxes sit on the right of the home screen.

**JSONL / crash:** none.

**Root cause:** (1) Hosts without bracketed paste deliver the payload as a flood of `Key::Char`, so collapse-to-chip never runs and the box wraps a wall of text. (2) Home side gutters / session→home cells are spaces in *both* ratatui frames. Skip-diff therefore never overwrites the emulator's paste echo (or leftover session sidebar chrome) on the PTY. Submit changes enough cells to hide it; a fresh empty home does not.

**Fix:** `coalesce_unbracketed_paste` folds a key-flood that `should_collapse` into one `Event::Paste`. `request_full_clear(2)` on submit, session switch, new session, and clear-session.

**Prevention:** Any layout jump onto a screen whose unused cells are spaces (home gutters) needs `terminal.clear()`, not just `fill_blank`. Unbracketed paste is not `Event::Paste`.

### 2026-08-24 — model picker keeps the previous provider's API key

**Symptom:** Logged in to `google-antigravity`, footer shows
`google-antigravity/gemini-3.5-flash-low`, new process / new session
fails on first turn: `Code Assist loadCodeAssist (401 Unauthorized):
Expected OAuth 2 access token`. Displayed as "check API key".

**JSONL:** `turn.error` after `tui.starting` with a *different* default
provider (e.g. `tektik`). Token `expires_at` still in the future.

**Root cause:** `apply_model_choice` only overwrote `api_key` when the
*new* provider had a config/env key. Switching from a key-bearing
default to an OAuth-only provider left the previous bearer in place.
Code Assist then got a non-Google token. `/new` does not reload
credentials, so the stale key survived.

**Fix:** on provider change, drop the old key (and unregister
`oauth_refresh` for the previous provider); load env/config for the
new one, else OAuth. Same rebind on `/models provider/model`, `/connect`,
and first-turn lazy fill. Code Assist 401 copy points at `auth login`.

**Prevention:** never reuse `api_key` across providers. A picker change
that only updates the chrome (footer) is not a credential switch.

### 2026-08-23 — `String::truncate` panics mid-UTF-8 in session memory

**Symptom:** TUI abort: `assertion failed: self.is_char_boundary(new_len)` at
`crates/memory/src/service.rs` while indexing a turn. Crash file
`crash-20260823T214444.033.txt`.

**JSONL / crash:** panic in `tokio-rt-worker`; location is the 2000-byte clip
in `MemoryService::index_session_turn`.

**Root cause:** `clip.truncate(2000)` is a **byte** cap. A 2-byte char
(`ç`, emoji, CJK) that straddles offset 2000 is not a char boundary;
`String::truncate` asserts.

**Fix:** `clip.truncate(clip.floor_char_boundary(MAX))`. Test uses a
payload whose 2000th byte sits inside `ç`.

**Prevention:** Never `String::truncate(n)` / `&s[..n]` on user text
without `is_char_boundary` / `floor_char_boundary`. Byte length ≠ char
length.

### 2026-08-20 — core 100% floor misses `save_todos` parent `if let`

**Symptom:** `Coverage (line floor)` fails `whycodes-core` with
`Uncovered Lines: crates/core/src/todo.rs: 130` (closing `}` of
`if let Some(parent) = path.parent()`). `Test (linux)` is green.

**JSONL / crash:** none.

**Root cause:** `todos_path` is always `working_dir/.whycodes/...`, so
`Path::parent()` is `Some`. llvm-cov attributes the unused `None` arm to
the closing brace (same shape as the sandbox `SSH_AUTH_SOCK` miss).

**Fix:** `create_dir_all(path.parent().unwrap_or(working_dir))` — no
`if let`. Serialize via `serde_json::Value` + eager `unwrap_or(to_string())`
so there is no dead `unwrap_or_else` closure for llvm-cov to miss.

**Prevention:** On 100% crates, do not `if let Some(parent) = path.parent()`
when the path is constructed with a directory prefix. Prefer `unwrap_or`
on a known fallback so there is no uncovered else brace.

### 2026-08-20 — TUI has no default max-turns cap (Grok parity)

**Symptom:** After ~19 minutes / 25 LLM steps a TUI turn died with
`Agent error: Exceeded maximum turns (25)` even though the task was still
in progress.

**JSONL / crash:** none (user-facing `Error::Agent`).

**Root cause:** `whycodes run` defaulted `-t/--max-turns` to 25 and the TUI
passed that into `Agent::run_turn`. Grok's `--max-turns` is **headless-only**;
the interactive TUI is unlimited and ignores the flag.

**Fix:** `max_turns: Option<usize>` (`None` = unlimited). Interactive TUI /
`--plain` REPL drop `--max-turns` with a stderr warning. `generate` and
`--format json|stream-json` still honour an explicit cap. Subagents keep
their own default (15).

**Prevention:** Do not put a default numeric turn cap on the interactive
agent loop. Loop protection is doom-loop + cancel, not a 25-step ceiling.

### 2026-08-25 — Long paste leftover sits *left of* the home prompt

**Symptom:** Pasting a long prompt on the home screen leaves the first lines of the paste visible to the left of the boxed input. Backspace / delete does not erase the ghost.

**JSONL / crash:** none.

**Root cause:** Home prompt is centered (`center_prompt_area`, ~70% / 75-col cap). Bracketed paste (or a key-flood paste) is echoed by the emulator at column 0. Ratatui skip-diffs spaces, so the side gutters never rewrite. `terminal.clear()` ran only on `Event::Paste`; typing Backspace after that is a Key event, so the leftover stayed.

**Fix:** `fill_blank` the left/right gutters before painting the centered box. `TuiApp::pending_full_clears` + `request_full_clear(n)` so paste requests two frames (echo can land *after* `Event::Paste`) and backspace/delete/clear request one. Detect unbracketed paste as a flood of ≥24 `Key::Char` in one drained batch.

**Prevention:** Centered widgets must own the unused columns. Any PTY write outside ratatui (paste echo) needs `terminal.clear()` *and* a follow-up frame. Editing the prompt after a paste is not a Paste event.

### 2026-08-20 — Long paste leftover sits beside the prompt

**Symptom:** After pasting a long prompt (then submitting / continuing the turn), the first lines of the paste stay visible in the 2-row gap above the input box (`Workspace line coverage…` next to `…`).

**JSONL / crash:** none.

**Root cause:** Bracketed paste is echoed by the emulator onto the alt-screen (or scrolls it) before `Event::Paste` is handled. Ratatui only diffs its own buffers. The prompt's `OUTER_TOP_GAP` / `CHAT_GAP` / side pad are spaces in *both* frames, so the diff never overwrites the echo. `Block::style(bg)` also only tints cells — it does not replace the symbol.

**Fix:** `terminal.clear()` on paste / resize / focus-gained before the next draw. `fill_blank` owns the full frame, the prompt gap, `CHAT_GAP`, and session side/bottom pads.

**Prevention:** Unpainted breathing-room rows are a paste-ghost magnet. After any event that can write to the PTY outside ratatui, full-clear. Do not rely on `Block` to erase glyphs.

### 2026-08-19 — xAI OAuth tokens are rejected by api.x.ai

**Symptom:** `whycodes auth login xai` succeeds, then the first turn shows `Authentication failed — check API key`.

**JSONL / crash:** none.

**Root cause:** Console keys (`xai-…`) authorize `api.x.ai`. SuperGrok / X Premium OAuth tokens authorize the Grok Build proxy `https://cli-chat-proxy.grok.com/v1` with `X-XAI-Token-Auth: xai-grok-cli`. Sending the subscription token to `api.x.ai/v1/chat/completions` is a 401.

**Fix:** `XaiProvider` routes non-`xai-` credentials to the proxy (chat completions + Grok CLI identity: `User-Agent: grok-shell/…`, `x-grok-client-identifier`, `X-XAI-Token-Auth`). A whycodes UA / GitHub `HTTP-Referer` yields `upstream=Unauthenticated, reason=no auth context` even with a fresh token. API keys stay on `api.x.ai`.

**Prevention:** Do not treat Grok account login as an `XAI_API_KEY`. Do not send `client_identity::post` (whycodes Referer) to `cli-chat-proxy.grok.com`.

### 2026-08-19 — Coverage flake: `project_path_uses_configured_or_cwd` vs chdir

**Symptom:** `Coverage (line floor)` fails `tests::project_path_uses_configured_or_cwd` (`left` a `/tmp/.tmp*` dir, `right` `crates/config`). `Test (linux)` is green.

**JSONL / crash:** none.

**Root cause:** `project_path_falls_back_when_cwd_gone` `set_current_dir`s a tempfile under `ENV_LOCK`. The other test read cwd twice without the lock, so a parallel thread could chdir between the snapshot and `Config::project_path()`.

**Fix:** Hold `lock_env()` in `project_path_uses_configured_or_cwd`.

**Prevention:** Any test that reads `current_dir()` in this crate must take `ENV_LOCK`. Process cwd is global to the test binary.

### 2026-08-19 — xAI loopback login is CORS/PNA, not only a 302

**Symptom:** Browser Grok/xAI sign-in would hang or fail if the loopback waiter treated the first TCP request as the code.

**JSONL / crash:** none.

**Root cause:** Grok Build's public client (`auth.x.ai`) lets the accounts app at `https://accounts.x.ai` fetch `http://127.0.0.1:<port>/callback` (Chrome Private Network Access). That starts with OPTIONS, and the GET needs `Access-Control-Allow-Private-Network`. A one-shot HTTP/1.1 accept (favicon / preflight) would miss the `code`.

**Fix:** `wait_for_callback` ignores OPTIONS and requests without `code`/`error`, answers CORS/PNA, and uses `127.0.0.1` in the xAI redirect URI. API calls send `X-XAI-Token-Auth: xai-grok-cli` for JWT subscription tokens.

**Prevention:** Do not collapse the loopback waiter to a single accept. Do not advertise `localhost` for a client registered as `127.0.0.1`.

### 2026-08-19 — Streaming code fence was O(N) Line clones per frame

**Symptom:** A long streamed ` ```rust ` dump made the TUI hitch. Idle startup/scroll were already cheap; the live bubble felt slow.

**JSONL / crash:** none.

**Root cause:** `last_checkpoint` cannot freeze inside an open fence. Every 40 ms frame re-parsed the whole tail, `highlight_code_spans` cloned every committed span, `render_code` rebuilt every ratatui `Line`, and `IncrementalMarkdown::render` cloned the frozen prefix into a new `Vec`. Layout then painted the live bubble again (2×).

**Fix:** Keep committed fence rows in `IncrementalMarkdown.buf`. Highlight via `with_open_code_spans` (no prefix clone). Paint live markdown by reference (`prefix` + `stream_md.lines()`).

**Prevention:** Never re-wrap a growing fence from scratch. Do not `clone()` frozen `Line`s on the stream frame. Do not concatenate the live answer into a throwaway `Vec` just to measure height.

### 2026-08-19 — `/compact` is Grok full-replace, not a 150k no-op

**Symptom:** `/compact` printed `Compacted N → N` and left the chat unchanged unless the session was already over ~112k tokens.

**JSONL / crash:** none.

**Root cause:** Manual compact reused `Session::compact(compaction_threshold)` (drop-oldest only when over ¾ of 150k). TUI did not reload the view. LLM summary ran only on auto-compact after a drop.

**Fix:** `/compact [context]` always full-replaces like grok-build: LLM structured summary (session model), keep last real user query + current-turn tail, prepend the continuation carrier, reload TUI from the session. Local stub when LLM is off or fails. Auto-compact uses the same path at `compaction_threshold`.

**Prevention:** Do not gate manual `/compact` on the auto-compact token threshold. Do not skip `load_messages_from_session` after a compact that mutates `session.messages`.

### 2026-08-19 — Compact summary hid behind the user-prompt 3-line fold

**Symptom:** After `/compact`, the chat showed `❯ This session is being continued…` and ` …`. The 9-section summary was not visible.

**JSONL / crash:** none.

**Root cause:** The model carrier is a user message. TUI paints user prompts like Grok `UserPromptBlock` (3 lines + ellipsis). The long preamble filled the cap. Grok does not put that carrier in the scrollback — it paints a session event ("Compaction completed" / "Context compacted: 48.8k → 27.1k") and keeps the summary model-only.

**Fix:** `chat_messages_from_session` maps compact carriers to a system card titled `Conversation compacted` with the summary body (preamble stripped). System callouts wrap so the sections stay readable.

**Prevention:** Compact carriers (`This session is being continued` / `[Compacted`) must not render as `ChatRole::User` prompts.

### 2026-08-19 — `/compact` froze the TUI

**Symptom:** After `/compact` the pager stopped taking keys / redraws until the LLM returned (or the 60s timeout).

**JSONL / crash:** none.

**Root cause:** `handle_slash` awaited `Agent::compact_session` on the event loop. That LLM complete can take tens of seconds (timeout 60s, retries ~90s). Grok runs compact as `CommandRunning` so the pager keeps pumping.

**Fix:** `/compact` only queues `SessionRuntime.pending_compact`. The loop spawns it like a turn (`take_turn_owner` + `TurnOutcome::Compact`). Esc still force-stops.

**Prevention:** Never `.await` an LLM call from `handle_slash` or any other path that sits on the TUI poll loop.

### 2026-08-19 — `apply_resume_found_missing_and_latest` flakes on shared `WHYCODES_HOME`

**Symptom:** `Test (linux)` fails `run::tests::apply_resume_found_missing_and_latest`: toast does not contain `No saved`. Isolated re-run is green.

**JSONL / crash:** none.

**Root cause:** `isolate_home()` uses a process-wide OnceLock temp dir. Parallel tests persist sessions into the same store, so `RESUME_LATEST` resumes a sibling session instead of the empty-store warning.

**Fix:** That test uses `isolate_home_fresh()` — a mutex plus a unique temp dir held for the test body.

**Prevention:** Tests that assert on an empty session store must not share `WHYCODES_HOME` with other tests.

### 2026-08-19 — Coverage flake: nucleo `rearm` tick(5) is too short under llvm-cov

**Symptom:** `Coverage (line floor)` fails `picker_flow_over_real_index` with `Ready { total: 4 }` and `matches=[]`. Isolated re-run and the `Test` job are green. Same shape as the 2026-08-16 picker note.

**JSONL / crash:** none.

**Root cause:** Idle-empty recovery called `nucleo.tick(5)`. Under llvm-cov / parallel load that times out before workers publish. `set_query` on the same pattern also returned without a tick, so a missed notify never recovered.

**Fix:** `rearm` / `query_blocking` share a 50 ms settle. Same-pattern `set_query` still `tick(0)`. Coverage job keeps `--skip picker_flow_over_real_index`.

**Prevention:** Do not lower the settle below `query_blocking`. Do not make `query_now` wait on `rearm` — that blocks a keystroke.

### 2026-08-19 — Keyboard Shortcuts popup is 70%/max 80, not 90%

**Symptom:** whycodes dialogs were stretched to 90% of the terminal after copying the generic `ModalSizing::large()` numbers. They did not look like Grok's Ctrl+. cheatsheet.

**JSONL / crash:** none.

**Root cause:** Grok's keyboard shortcuts modal uses `shortcuts_help::modal_sizing`: `width_pct=0.70`, `max_width=80`, `min_width=44`, `v_margin=4`, `footer_lines=2`. Rows are `◆` + right-aligned detail with a selected-row wash — not `▸` + accent text. 90% is the import-claude / form size, not the cheatsheet.

**Fix:** `DialogSizing::popup()` matches those numbers. List/help rows share `paint_picker_row`. Help is titled "Keyboard Shortcuts" with `/ to search`.

**Prevention:** Do not bump dialog `percent_x` to 90 to "match Grok". Use `popup_rect` / `DialogSizing::popup()`. Test: `popup_rect_matches_grok_shortcuts_sizing`.

---

### 2026-08-18 — Phone portrait popups + OSK resize

**Symptom:** On a vertical phone PTY, help/confirm/select popups were a thin percentage box. Opening/closing the on-screen keyboard left a garbled TUI until the next key.

**Root cause:** `centered_rect` used only `%` of the frame (50% of 40 cols = 20). ratatui autoresizes on `draw`; a `Resize` during a long idle poll left the internal buffer at the old size.

**Fix:** Floor modal size to 36×10 (clamped to the frame). Narrow sessions use `SIDE_PAD_NARROW`. `handle_event(Resize)` marks dirty; the loop `terminal.resize`s before the next paint.

**Prevention:** `centered_rect_expands_on_phone_portrait`, `narrow_session_uses_tighter_side_margin`, resize dirty assert.

---

### 2026-08-18 — Split/quarter pane: Grok popup formula + drop sidebar

**Symptom:** A tmux/iTerm split (½ or ¼ of ~80 cols) left confirm/help as a thin % box and still reserved a 24-col sidebar, crushing the transcript.

**Root cause:** Dialogs passed 50–70% into `centered_rect`. Grok uses `max(percent of outer, min).min(outer)` at ~90%. Sidebar gated on `width >= 36`, so a 40-col pane still paid for the rail.

**Fix:** Shared `layout::popup_dim`. Callers use 90% (Grok). Compact inner pad under 48×14. Sidebar only when `width >= 72` *and* chat keeps 32 cols.

**Prevention:** `popup_dim_matches_grok_max_percent_then_clamp`, `split_pane_hides_sidebar_so_chat_keeps_the_width`.

---

### 2026-08-18 — Chat scrollbar must not overlay transcript text

**Symptom:** Overflowing chat painted the solid scrollbar on the last wrap column, so glyphs sat under the bar.

**Root cause:** `render_session` used `content_width = area.width` then drew the bar over `x + width - 1`.

**Fix:** When `total > viewport`, wrap at `width − SCROLLBAR_GUTTER` and paint the bar in that reserved column. Hit box matches the gutter (not two cells into the text).

**Prevention:** `overflowing_chat_does_not_paint_text_under_scrollbar` in `chat_scroll_tests.rs`.

---

### 2026-08-17 — Competitor audit: draw recovery, resize coalesce, git timeout

**Symptom / gap:** jcode keeps the TUI alive after a widget panic; Grok never `malloc_trim`s inside a frame; a wedged `git` or a resize flood can stall whycodes.

**Fix:** `catch_unwind` around `render_inner` + fallback banner. Coalesce `Resize` to the last size in a batch. `git rev-parse` capped at 250 ms (child killed). Turn-done heap trim is `request_release_after_draw` (Grok), drained after `terminal.draw`.

**Prevention:** Recovered-frame paint test. Resize-coalesce keeps one event. Sleep+timeout test must finish ≪ 2s.

---

### 2026-08-17 — jcode: toasts are not animation; deep idle + malloc_trim

**Symptom:** A visible toast (or any static chrome) kept the loop at 40 ms full paints. jcode measured the same class of bug at ~180 frames per 3s notice and 0.22 CPU cores on decorative 60 fps.

**Root cause:** `animate = agent_busy || !toasts.is_empty()`. Toasts are event-driven text; they already `mark_dirty` on push/prune.

**Fix:** `redraw_schedule` — only spinner/stream/subagents animate. Idle 500 ms; 30s without a real key/click/paste → 5s deep idle. Mouse *move* does not reset the clock. Linux `malloc_trim(0)` after each turn and once per 60s idle (jcode `idle_heap_release`).

**Prevention:** A visible toast must poll at idle, not animation cadence. Assert in `redraw_schedule` tests.

---

### 2026-08-17 — Port Grok paint-window + markdown checkpoints + packed cells

**Symptom:** Long transcripts walked every message each frame. Streaming a reply re-parsed the whole bubble (O(N²)). Mouse-select allocated one `String` per cell.

**Root cause:** Grok Build already solved these: `partition_point` on prefix-sum `virtual_y`, freeze markdown at `last_checkpoint`, pack the selection grid as UTF-8 + `u32` offsets.

**Fix:** `visible_message_range` (two `partition_point`s). `IncrementalMarkdown` on the live assistant bubble. `CellGrid` for `screen_cells`.

**Prevention:** Viewport → message must be O(log n). Live markdown must not re-parse frozen prefix. Selection snapshot must not heap-allocate per cell.

---

### 2026-08-17 — Sessions dialog / parked drain burned idle CPU

**Symptom:** Opening the live-session dashboard kept the TUI at ~25 fps full paints with nothing changing. Several parked sessions made idle ticks allocate.

**Root cause:** The loop `mark_dirty()`’d whenever `DialogKind::Sessions` was open. The picker rebuild also re-hit SQLite when the list was live-only. `drain_background_runtime` built a `TuiApp::new` (applies the default syntax theme → can flush highlight caches) and cloned the whole transcript every tick, even with an empty event channel.

**Fix:** Dirty dashboard/picker only when rows change. Skip DB on picker refresh. Idle drain is `try_recv` only; events `adopt_view`/`yield_view` a detached app (`from_config`, no theme swap). Session switch moves the transcript (live snapshot stays empty — dashboard preview reads `app.messages`). Closed highlight memo is LRU, not nuke-all at 64.

**Prevention:** Idle poll must stay 500 ms with a modal open and no row change. Parked drain with no events must not reallocate `view.messages`. `TuiApp::new` is only for the visible app. After switch, do not read `rt.view.messages` for the active row.

---

### 2026-08-17 — Answer sat above tools; chat flush on [stop]

**Symptom:** During a tool-using turn the written reply stayed at the top of the bubble (tools looked fine). The last transcript line sat on the `[stop]` row. Bubbles had almost no left/right margin. Fenced code often looked uncoloured.

**Root cause:** `render_message` dumped all of `msg.content` *before* the first `ToolUse`, so auto-scroll (offset 0) showed the tool cards and left the growing answer above the fold. Session split was chat · turn · prompt with no gap; `SIDE_PAD` was 2. Code-band wash used `set_style` on a reconstructed `Style` (can drop RGB token colours). Untagged fences never guessed a grammar.

**Fix:** Paint thinking → tools → answer. `CHAT_GAP` blank row between chat and stop/prompt. `SIDE_PAD = 4`. Band wash uses `Cell::set_bg`. Untagged fences try `find_syntax_by_first_line`. Do not cache height on the live streaming bubble.

**Prevention:** Assert `◆` / Read appears above `AFTER_TOOLS_ANSWER`. Full-shell paint: `stop.y >= chat.bottom + CHAT_GAP`. Tagged rust fence paint has ≥2 distinct RGB token colours.

---

### 2026-08-17 — Code token colours come from Grok's tmTheme

**Symptom:** Fenced Rust/JS looked like Tokyo Night even on the default dark UI. Grok Night uses a cooler grey/magenta palette (`#b2b2b2` fg, `#51597d` comments).

**Fix:** Embed `grok-night.tmTheme` / `grok-day.tmTheme`. Default + most dark themes use Night; light themes use Day; Tokyo Night TUI keeps `tokyo-night.tmTheme`. `set_syntax_theme` clears highlight caches.

**Prevention:** Assert Night vs Day produce different `fn main()` colours.

---

### 2026-08-17 — Fenced code matches Grok (band + line numbers)

**Symptom:** Assistant ` ``` ` blocks used `┌ rust` / `│` / `└` chrome. Next to Grok they looked like a broken box.

**Fix:** Elevated `status_bar_bg` band, dim language chip, right-aligned line numbers, syntax highlight on the band. No box drawing. Diff fences keep add/remove wash on the same layout.

**Prevention:** `fenced_code_is_banded_labelled_and_numbered` forbids `┌└`.

---

### 2026-08-17 — Thinking / Run left rail matches Grok status pulse

**Symptom:** Thinking had a faint wave; Run had no left ┃. Grok paints a full-height accent whose color follows the block (purple thinking, green run, red error) and a 2-row hot band walks down the column while the work is live.

**Fix:** Shared `accent_rail_style`. Thinking header always has ┃. Execute header + body share a status-colored ┃ that pulses while `result` is `None`.

**Prevention:** Unit-test running Run rails differ across spinner frames, and failed Run uses `palette.error`.

---

### 2026-08-17 — Sticky user header, waving thought rail, collapsed tools

**Symptom:** Long tool dumps filled the transcript. Scrolling past a prompt lost which turn you were in. Live thinking rails sat still.

**Root cause:** Tools always painted a 12-line preview. No `sticky_headers`. Thinking `┃` used a static dim color.

**Fix:** Pin the last scrolled-past user ❯ band at the chat top. Wave the live thinking rail with `spinner_frame`. Collapsed tools are header-only (`›` to open). Shell bodies use Grok `first 2 + last 3`.

**Prevention:** Paint-test overflow sticky headers. Collapsed `tool_block` must be one line. Execute expand must hide the middle dump.

---

### 2026-08-17 — Thinking / tool chrome must follow Grok pager defaults

**Symptom:** Reply clocks matched; thinking still said `Thinking · 1.4s` and tools used lowercase `read · path` / `(l expand)`.

**Root cause:** Grok live thinking is `Thinking...` with the timer on the right. Finished is `Thought for Xs` + `›`. Tools use gerunds while open (`Reading`) and labels when done (`Read`), with `◆` and `›`.

**Fix:** Paint that header/tool chrome. Do not invent a mid-dot timer or `(l expand)`.

**Prevention:** Unit-test the live thinking header and the `◆ Read` / `◆ Run` verbs.

---

### 2026-08-17 — Agent replies must carry the same clock as Grok

**Symptom:** User ❯ bands had `August 17, 14:32` on the right; the answer body did not. Grok settings copy is "clock time next to user messages and agent".

**Root cause:** `/timestamps` only stamped the user band and the `Worked for` footer. Agent markdown was unstamped. Tools also lacked Grok's `◆` bullet and folded thinking used `(e expand)` instead of `›`.

**Fix:** Stamp the first answer line with `line_with_right`. Tool headers start with `◆ `. Folded thinking puts `›` on the right of the header.

**Prevention:** Paint-test an assistant reply through the full shell; the marker row must contain `HH:MM` to the right of the text.

---

### 2026-08-17 — First user bubble missing its clock

**Symptom:** Scrolling to the first prompt in chat history showed no time on the right of the ❯ band. Later short follow-ups had the clock.

**Root cause:** `line_with_right` dropped the clock when left + `August 17, 14:32` did not fit. The first user prompt is often the longest line. A transcript scrollbar also painted over the last column, eating the minutes.

**Fix:** Never drop the clock — truncate the left side instead. Leave a 1-col gutter so the scrollbar cannot cover it.

**Prevention:** Paint-test the first user bubble through the full shell. A too-wide left side must still keep `HH:MM`.

---

### 2026-08-17 — CI Budgets + index watcher flake

**Symptom:** `Budgets` red on `sh scripts/test_tui_term_matrix.sh` (no `ok`). `Test (linux)` red on `whycodes-index::watcher_picks_up_changes` (`create must be indexed`, 5 vs 6).

**Root cause:** `--dry-run` skipped hosts whose emulator was not on PATH, so CI (no Alacritty) never printed the argv the test locks. Index `wait_ready` flipped Ready at the end of `full_scan`, *then* `notify` was armed — a create in that window was lost.

**Fix:** Dry-run prints argv even when the binary is missing. Scanner marks Ready only after `watch::spawn`. Test polls for the new path, not just `len()`.

**Prevention:** Dry-run / `--list` paths must not require the real emulator. Never signal “ready for live updates” before the watcher is installed.

---

### 2026-08-17 — Header–chat gap too tight; scroll still hitchy

**Symptom:** Transcript sat almost flush under the `whycodes` header (one blank row). Wheel/trackpad scroll was still not fluid after the previous drain-only fix.

**Root cause:** `TOP_PAD` was 1. Paint still cloned every visible `Line`/`Span`/`String`, wiped every cell then stamped (2× writes), and re-parsed the selected bubble. Wheel notches in a drained batch were applied one `handle_mouse` at a time instead of one offset change.

**Fix:** `TOP_PAD = 2`. Paint cached rows by `Arc` reference; one-pass row fill (no area wipe); caret is paint-time only. Coalesce wheel events to a single `scroll_rows`. Skip `mark_dirty` when the offset does not change.

**Prevention:** Do not clone transcript `Line`s on the scroll frame. Do not key or invalidate paint on selection — overlay the caret.

---

### 2026-08-16 — Chat freeze on mouse-wheel scroll

**Symptom:** Flicking the wheel / trackpad over a long transcript made the TUI hang for a second or more. Scrolling during a live turn was worst.

**Root cause:** Two stacked costs.

1. The event loop read **one** crossterm event per `terminal.draw`. A trackpad flick is dozens of `ScrollUp`/`ScrollDown` events; each frame re-laid and stamped the chat.
2. Line/layout caches were keyed by `(width, app.is_busy())`. Starting or ending a turn flipped `busy` and evicted **every** finished bubble. The next scroll re-parsed all markdown. Paint also `clone()`d each visible message's full `line_cache` before slicing.

**Fix:** Drain up to 64 queued events before the next paint (mouse-move alone does not `mark_dirty`). Cache key is `(width, message_is_closed)` so a new turn does not evict history. Copy only the viewport slice of a cached bubble.

**Prevention:** Never key a per-message paint cache on a global busy flag. Wheel/trackpad input must be drained, not painted 1:1.

---

### 2026-08-16 — Chat painted into the status header

**Symptom:** Session transcript (user band / first visible line) sat flush on the `whycodes` header row. Scrolling to the top made the elevated user band look like it had overflowed into the chrome / safe area.

**Root cause:** Shell split was header · body · footer inside `inset_safe`. Body started on the next cell after the 1-row status bar. Session inset applied `SIDE_PAD` + `BOTTOM_PAD` but no top gap, so `SparseLines` wiped and stamped chat at `body.y` — the header’s neighbour.

**Fix:** `layout::TOP_PAD` + `layout::below_header` on the body before home/session/sidebar paint. Header row and the `SAFE_TOP` terminal edge stay empty of chat. Paint tests lock the invariant (bottom-pinned and scrolled-to-top).

**Prevention:** Any new shell region that draws above the prompt must go through `inset_safe` and `below_header`. Do not start a transcript wipe at `outer[1].y`.

---

### 2026-08-13 — CI Budgets + Clippy keep failing feature pushes

**Symptom:** GitHub: “Some checks were not successful — 2 failing (Budgets, Check & Lint), 1 queued, 1 in progress, 2 skipped.” Happened on `feat(serve)` and again on `feat(latency)` (race + response cache). Local `cargo test` was green.

**JSONL / crash:** none — this is the `budgets` + `check` CI jobs, not a runtime bug.

**Root cause:** New code added scanner hits and a workspace edge without touching the budget JSONs, and nobody ran the CI scripts locally.

1. **Swallowed errors** (`server` budget 0, `llm` budget 4):
   - `crates/server/src/routes.rs` — four `let _ = tx.send(TurnEvent::Status(...))` after a chat turn.
   - `crates/llm` — `return u32::try_from(n).ok();` / `s.parse().ok();` in `model_catalog.rs` (false positive: `return ….ok();` still matches `\.ok\(\);`), `Err(_) =>` on `tokio::time::timeout` in `race.rs` + `transport.rs`, `Err(_) => return` on a poisoned `Mutex` in `response_cache.rs`.
2. **Hidden second fail:** Budgets steps are sequential. Swallowed-error failing skipped **Dependency boundaries**. After that was fixed, the next run failed: `server gained a dependency: auth, index, storage` — real edges from the serve daemon, never registered.
3. **Clippy `-D warnings`:** `turn_event_json` used `if let Some(msg) = s.strip_prefix("error:") { … } else { return None; }` → `clippy::question_mark`.

**Fix:** Handle, don’t swallow — `emit_status` logs a closed channel; name timeout/`Elapsed`; warn on lock poison; rewrite catalog `as_u32` to `if let Ok`. Clippy arm → `let msg = s.strip_prefix("error:")?;`. Lock budgets (`llm` 4→0, `memory` 9→8). Register `server → auth, index, storage` in `dependency_boundaries.json`.

**Prevention:** Rule 8. After any `.rs` / `Cargo.toml` edit, run the three budget scripts **and** clippy `-D warnings` before push. Do not treat “I didn’t add a crate” as a skip — a single `let _ =` or a new `whycodes-*` dep is enough. Do not raise a budget to paper over `let _ =` / `Err(_)` when the error can be named or logged.

---

### 2026-08-07 — Stop stuck on "Cancelling…"

**Symptom:** Esc / `[stop]` showed `Cancelling…` and never returned to idle; second presses did nothing useful.

**Root cause:** Cancel was a cooperative `AtomicBool` checked only *after* the next LLM SSE event or *between* tools. Idle stream waits and long tools ignored the flag. The TUI never aborted the turn `JoinHandle`, so a hung HTTP body pinned `agent_busy` forever (agent/session still moved into the task).

**Fix:** `wait_until_cancelled` + `tokio::select!` on stream open, stream body, and tool batches. TUI: `begin_cancel` denies pending permission/question waiters; force-stop after 1.2s or on second Esc/[stop]/ abort join handle, restore session backup, rebuild agent if needed, clear busy.

**Prevention:** Never rely on cancel-only-between-events. Any long `.await` in the agent turn must race `wait_until_cancelled`. TUI must keep a join handle + session backup for hard abort.

---

### 2026-08-06 — Long paste overflowed the prompt box

**Symptom:** Pasting a long paragraph into the home/session prompt made text look like it spilled past the input box (tall reflow, clipped chrome).

**Root cause:** (1) Collapse thresholds were high (3 lines / 800 chars) so mid-size pastes stayed inline and wrapped to 8 rows. (2) `prompt_height.min(height/2)` allocated less than the box needed, crushing borders. (3) `wrap_text` soft-break math could leave residual width wrong after a space break.

**Fix:** Collapse at ≥2 lines or ≥160 chars; allocate prompt height with `needed.min(area - logo reserve)` instead of half-screen; harden `wrap_text` (recompute width after soft break, wide-glyph alone-row); clamp painted rows to `text_w`.

**Prevention:** Any change to paste/prompt height must keep collapse thresholds in sync with `MAX_INPUT_ROWS` and never cap prompt height below chrome+1 without a residual min for the box.

---

### 2026-08-06 — Question panel: bottom dock, navigate, copy

**Symptom:** Questionnaire felt like a center modal, not Grok’s bottom line-by-line picker. No way to revisit a previous question in a multi-q set or copy the prompt/options.

**Fix:** Bottom-docked `dialog_frame_placed(Bottom)`; one selectable row per option (label + description) with mouse hit → index; `←/→`/`[/]`/`h`/`l` navigate questions (forward only when answered); `y`/`c` copies full questionnaire via clipboard; click single-select confirms (multi toggles). Run loop drains `pending_question_answers` like `question_dismissed`.

**Prevention:** Keep option hit-testing 1 row = 1 option; multi-line previews must sit *outside* the list hit rect. Always complete the oneshot on cancel *and* on mouse-finish.

---

### 2026-08-06 — Interactive `question` tool (Grok-style questionnaire)

**Symptom:** Agent needed clarification; TUI had no way to answer with options. Old `question` wrote to stderr and blocked on stdin (broken under alt-screen).

**Fix:** Channel prompter (like permissions): `ChannelQuestionPrompter` → TUI modal with ↑/↓, Enter, Space multi-select, Other free-text, digit shortcuts. Schema supports Grok-style `questions[]` + `options[{label,description}]` + `multi_select`, plus legacy `question`/`choices`. Config: `[tools.question] timeout_enabled` / `timeout_secs` (default 30m). Core tool profile includes `question`.

**Prevention:** Never read stdin from tools under TUI raw mode; use oneshot/channel and always complete the reply on Esc/`[✗]`/exit.

---

### 2026-08-06 — Tool results: minified JSON flood (webfetch / package.json)

**Symptom:** After a tool turn, the transcript looks like a wall of mid-JSON with `┃` rails (`"exports":…`, `_npmUser`, …) ending in `Worked for Xs` — easy to mistake for the assistant answer. One minified line wrapped across the entire preview budget.

**Root cause:** `tool_result_plain` soft-wrapped every logical line. A single huge npm/`package.json` line ate all `TOOL_RESULT_PREVIEW` visual rows. No display-time JSON pretty-print.

**Fix:** `prettify_tool_result` (whole body, webfetch `\n\n` envelope, or first parseable `{`/`[`); pure JSON → `Code("json")` highlight; plain path **hard-truncates** each logical line to one terminal row (`…`) instead of wrap-filling the budget.

**Prevention:** Never soft-wrap untrusted tool blobs into the preview budget; pretty-print JSON for display; one row per logical line when collapsed.

---

### 2026-08-06 — Intent: question vs change vs plan

**Symptom:** In build mode the model often edits when the user only asked a question, or skips planning on large design asks.

**Root cause:** No product-level intent layer — only free-form model judgment. Competitors use hard modes (Cursor Ask/Plan/Agent, OpenCode Build/Plan) plus prompt rules, not a silent ML router.

**Fix:** Three layers: (1) primary agents `build` / `plan` / `ask` with tool denylists; (2) prompt protocol in `prompts/{build,plan,ask}.txt`; (3) zero-LLM heuristic (`crates/agent/src/intent.rs`) injects ephemeral `<whycodes_intent>` on the **request** only (not session history) so system prompt cache stays stable. Config: `session.intent_guidance = auto|off|always`.

**UX + auth (same day):** `TurnEvent::Intent` → TUI badge (`[Q]` / `chg` / `plan` in header + prompt chrome); mode-mismatch **Warning** toast (8s TTL); Claude-style `authorize_tool` escalates mutators (edit/write/shell) to Confirm when turn intent is high-confidence question/plan. Read-only shell (`ls`, `git status`, …) still allowed. `intent_guidance=off` disables posture + auth escalate.

**Prevention:** Do not add a per-turn LLM intent classifier by default (latency + cache cost). Prefer mode gates + soft posture. Keep posture off the stored transcript. Never skip intent auth for shell just because risk said Allow.

---

### 2026-08-06 — Help modal could not copy text (split mouse path)

**Symptom:** Drag-select inside Help (`?`) did nothing; other popups could copy.

**Root cause:** Help used `AppMode::Help` with a separate mouse handler that only scrolled. It never set `dialog_modal_hit` / `dialog_close_hit` and never ran the select→clipboard path.

**Fix:** Every popup paints via `dialog_frame` + `apply_modal_chrome` / `apply_select_paint`. One `handle_modal_mouse` for dialog stack **and** Help: clip select, copy, `[✗]`, scrollbar.

**Prevention:** New overlays must call `apply_modal_chrome` (or select paint) and open under `modal_is_open()` — do not invent a parallel mouse path.

---

### 2026-08-06 — Permission popup: unreadable JSON + copy leaked background chat

**Symptom:** Tool permission dialog showed compact JSON (`{"command":"…"}` truncated mid-string). Drag-selecting inside the popup also copied chat text behind it (middle lines of a linear selection span full terminal width).

**Root cause:** (1) `PermissionAction::Ask` used `arguments.to_string()` + 200-char byte truncate. (2) Dialog mouse path tracked selection but did not clip extract/paint to the modal rect; `linear_cols` middle rows use columns `0..width-1`.

**Fix:** Pretty `format_permission_detail` / shell risk sections; dedicated `render_permission_dialog` layout; `dialog_modal_hit` + `text_from_cells_clipped` / `paint_ranges_clipped`; dialog drag copy only within modal.

**Prevention:** Any modal that owns focus must set `dialog_modal_hit` and pass it into selection extract/paint. Never dump compact JSON into a permission body.

---

### 2026-08-05 — Boot/TTFF: do not put Tokio before `--version`

**Symptom:** `whycodes --version` (Boot floor in comparison tables) paid multi-ms for a multi-thread Tokio runtime that never ran work. Windows baseline ~21 ms was mostly binary page-in + that setup.

**Root cause:** `#[tokio::main]` builds the runtime *before* the async body runs, so clap’s version exit still started worker threads. Full RELRO (`BIND_NOW`) resolved every relocation at start. 16 MB binary + mermaid-text + two-face packs meant more pages faulted in.

**Fix:** Sync `main` (early `--version`/`-V`, parse before runtime, current-thread for light cmds); release `panic=abort` + **full LTO**; Linux `.cargo/config.toml` `relro-level=partial`; drop unused tiktoken tables; mermaid + extended-syntax **opt-in** (`--features full`) → ~1.0 ms / 12 MB on Linux.

**Prevention:** Never restore `#[tokio::main]` or full RELRO on the CLI entry without re-measuring. Keep default features slim. Measure with `python scripts/bench_startup.py`.

---

### 2026-08-05 — Scrollbar thumb stuck near ~70% at true bottom

**Symptom:** Content at newest messages (`scroll_offset = 0`) but the thumb sat mid/lower track (~70%), not flush with the bottom.

**Root cause:** `ratatui::widgets::Scrollbar` uses  
`thumb_start = position * track / (content_length - 1 + viewport)`.  
With `position = view_start = total - height` that never reaches the track end.

**Fix:** Paint chat with our proportional `paint_scrollbar` (`offset * travel / max_off`), same geometry as drag hit-testing. At `view_start = max_off` the thumb is flush bottom.

**Prevention:** Do not use ratatui Scrollbar for bottom-anchored chat without remapping position into its odd end model.

---

### 2026-08-05 — Scrollbar “bottom” stopped a few rows short of newest

**Symptom:** Dragging/clicking the chat scrollbar to the bottom did not show the latest messages (`scroll_offset` stayed > 0).

**Root cause:** Track clicks used `grab = thumb_len/2`. On the last track cell the centered grab never produced `view_start == max_off`, so bottom-anchored `scroll_offset = max_off - view_start` stayed positive.

**Fix:** Snap track ends (and thumb flush with track bottom) to document top/bottom before converting to bottom-anchored offset. Tests assert bottom cell → offset 0 and painted latest marker.

**Prevention:** Any inverted (bottom-anchored) scrollbar must snap ends; do not rely on mid-thumb grab alone for edge positions.

---

### 2026-08-05 — Chat mouse scroll looked dead (SparseLines ghosts)

**Symptom:** On the main message list (no dialog), mouse wheel / scroll did nothing useful — content stayed put or looked garbled. Dialogs were fine.

**Root cause:** `SparseLines` only wrote non-empty spans into the ratatui buffer. After a scroll, shorter/empty rows never overwrote previous-frame cells, so old glyphs remained. Offset *did* change; the paint path made it look frozen.

**Fix:** Wipe the chat viewport to spaces + bg before stamping lines. Clipboard still trims pad. Wheel step scales with viewport; scrollbar drag kept from prior fix. Scroll metrics unified (`chat_scroll_metrics` / clamp on paint). `auto_scroll` stays true when offset remains 0 (no-op wheel on short chats).

**Tests:** `crates/tui/src/ui/chat_scroll_tests.rs` — geometry, clamp, wheel both directions, bar drag, dialog/help isolation, PgUp/Dn, full `TestBackend` paint + ghost check.

**Prevention:** Any widget that does not fill its area must clear first when the previous frame can leave symbols (ratatui double-buffer + diff).

---

### 2026-08-05 — Chat transcript scrollbar was decorative only

**Symptom:** Message list showed a scrollbar but mouse drag / track click did nothing.

**Root cause:** Session chat painted a ratatui `Scrollbar` with no hit box and no mouse branch.

**Fix:** `apply_chat_paint` + drag/track handling; wheel → `scroll_rows`.

**Prevention:** Painted scrollbars need hit rects.

---

### 2026-08-04 — Dialog scrollbar wheel + [✗] were decorative only

**Symptom:** Sessions (and other) picker: mouse wheel scrolled the chat behind the modal; top-right `[✗]` did nothing.

**Root cause:**
- `handle_mouse` always called `app.scroll_rows` — no dialog branch.
- `paint_close_button` was visual parity only; no hit rect, no click handler.

**Fix:**
- Paint stores `dialog_close_hit` / `dialog_list_hit` + scroll window on `TuiApp`.
- `handle_dialog_mouse`: wheel → `move_in_dialog`, click `[✗]` → dismiss, click row → select (+ confirm for Session/Model/Provider).
- Shared `close_button_rect` keeps glyph and hit target aligned.

**Prevention:** Any new modal that paints chrome via `dialog_frame` must publish hit boxes if it wants click/wheel; never assume Esc-only is enough when UI shows `[✗]`.

---

### 2026-08-04 — Image drag-drop / path paste on the prompt

**What:** Dragging an image onto the terminal (or pasting its path) attaches it to the next user turn as multimodal content.

**How:**
- `EnableBracketedPaste` so hosts deliver drop/paste as `Event::Paste` (not char spam).
- `images::classify_paste` detects existing image paths (quotes, `file://`, `~`).
- Staged on `TuiApp.pending_images`; chips in the prompt box; Backspace on empty buffer peels the last one.
- On submit: base64 `ContentBlock::Image` via `session.add_user_message_blocks` (OpenAI/Anthropic paths already serialize images).

**Limits:** 10 images/turn, 20 MB each; extensions png/jpg/gif/webp/bmp/tiff/svg/heic/avif/ico.

**Not covered (then):** Raw clipboard bitmap paste. **Now covered (2026-09-01):** Ctrl+V via `clipboard_image` (OS tools, stashed file, same `pending_images` chips).

---

### 2026-08-05 — Competitive latency pack (cache + parallel tools)

**What:** Match Claude Code / OpenCode / Codex / jcode latency patterns:

| Piece | Behavior |
|-------|----------|
| Anthropic cache (OpenCode auto) | last tool + system + **latest user message** (`llm/cache.rs`) |
| Parallel tools | fan-out read-safe tools; shell/mutators serial |
| Doom-loop | 3× same tool+args → refuse (OpenCode `doom_loop`) |
| Core tools | default `session.tool_profile = "core"` (~12 tools); `"full"` for all |
| Metrics | JSONL `turn.step` / `turn.done` (`ttft_ms`, `step_ms`, `tool_batch_ms`, cache tokens) |
| Model routing | trivial chat → `model_fast` or haiku/mini sibling |
| Perm queue | multi-ask VecDeque; parallel Ask-safe tools |
| Tool prune | every step: cap + older tools → 2k chars; compact when over threshold |
| Auto-compact | before each LLM step when over `compaction_threshold` |

**Roadmap:** [archive/plan-latency-competitors.md](archive/plan-latency-competitors.md)

**Prevention:** Do not drop latest-user cache breakpoint; do not reintroduce sequential-only tool loops or full tool dump as default.

---

### 2026-08-13 — First-token race + semantic response cache (P2)

**What:** Close latency P2 without double-billing the default path.

| Piece | Behavior |
|-------|----------|
| `session.model_race` | `off` (default) / `auto` (small sibling) / `provider/model`. Primary opens first; partner starts only after `race_after_ms` (800) with no text/thinking/tool token. First meaningful event wins; loser stream is dropped. |
| `session.response_cache` | `auto` (default). Process-local exact hash + hashed n-gram embed. **Tools-free requests only** (title, compact, trivial chat). Different `system` cannot semantic-hit. |

**Prevention:** Never cache a tool-using turn. Never start the race partner when primary already matches the partner model. Do not default `model_race` to `auto` (surprise haiku answers + extra prefill).

---

### 2026-08-06 — Answer on screen but "generating" keeps spinning (memory retain)

**Symptom:** Final assistant text is fully streamed; status strip still shows `generating Xs` for ~5–12s more. Logs: `turn.done` then silence until `auto-retained memories`.

**Root cause:** `Agent::run_turn_with_events` **awaited** post-turn `run_post_turn_retain` (heuristic + optional LLM extract up to 12s) **before** returning `Ok`. TUI only clears `agent_busy` / `AgentState::Generating` when `done_rx` fires.

**Fix:** `spawn_post_turn_retain` — fire-and-forget after `turn.done` (same pattern as title refine). Status "Remembered N…" may arrive after Idle → quiet toast, do not clobber "Worked for…".

**Prevention:** Never block `agent_busy` on niceties (title, memory retain, telemetry, catalog).

---

### 2026-08-05 — Slow turns / inflated "Worked for Xs" (HTTP + title refine)

**Symptom:** Simple chat ("selam") shows tens of seconds; first turn especially sluggish; multi-step tool loops feel cold every time.

**Root cause:**
1. `client_identity::http_client()` built a **new** `reqwest::Client` per request → no keep-alive / TLS session reuse.
2. TUI **awaited** LLM title refine before releasing `agent_busy` (second API call after every first turn).
3. Title refine also ran for trivial greetings where the offline heuristic is enough.

**Fix:**
- Process-wide `OnceLock` HTTP client (`pool_max_idle_per_host`, `tcp_nodelay`, keepalive).
- `Agent::spawn_title_refine` + TUI channel `(session_id, title)` — turn completes immediately; title applies async (with race buffer).
- `is_trivial_title_seed` skips refine for greetings/pings; 8s timeout on title complete.
- Stable sorted tool definitions (prompt-cache friendly).

**Prevention:** Never `Client::builder()` on the hot path. Never block `agent_busy` on niceties (title, telemetry).

---

### 2026-08-05 — "Worked for 28s" vs gateway Duration ~2s (catalog race)

**Symptom:** First turn ("selam") shows `Worked for 28s`; OmniRoute (or similar) panel reports request Duration ~1.8s. JSONL: `ttft_ms≈27953`, `step_ms≈28307`, and `GET /v1/models failed` exactly ~15s after `tui.first_frame`.

**Root cause:** TUI spawned `GET /v1/models` at open with a **15s** request timeout on the **same host** as chat. Gateways with low concurrency effectively serialize: catalog holds the slot; chat waits; wall clock ≫ model Duration. No `connect_timeout` on the shared client made dead hops worse (long SYN retries).

**Repro (outside whycodes):** hang/slow `/v1/models` + concurrent `/v1/chat/completions` → chat TTFT balloons; alone chat stays ~2–3s.

**Fix:**
- Do **not** fetch catalog at TUI open; queue after turn complete / model switch when `!agent_busy`.
- Catalog timeout **3s**; shared client `connect_timeout` **5s** (no client-wide body timeout — streams stay open).

**Prevention:** Never start non-critical gateway traffic concurrent with a user turn on the same base URL. Prefer local context fallback until idle.

---

### 2026-08-05 — Grok-style chrome pack (HitArea, turn strip, status bar)

**What:** Ported portable Grok Build TUI patterns (no xAI product chrome):

| Piece | Module |
|-------|--------|
| Sticky hit targets | `hit::HitArea` — paint sets `rect`, mouse sets `hovered` |
| Context bar | default `used/total` → hover 1/8 bar + `fmt_pct5` |
| Progress / urgency color | `ui/progress_bar.rs` |
| Composable right status | `ui/status_bar.rs` (`│` seps + hit map) |
| Turn strip | spinner · activity …… `⇣tokens` **`[stop]`** (mouse → `pending_cancel`) |
| Path underline on hover | `cwd_hit.hovered` |
| Slash mouse hover/click | `SlashSuggestState.hovered` + `list_hit` |

**Rule:** Never recompute hover after clearing `rect` mid-paint. Use `update_chrome_hover()`.

**Ship:** `cargo install --path crates/cli --force` (PATH binary, not only `cargo test`).

### 2026-08-05 — Context meter hover never swaps (always tokens)

**Symptom:** Bottom-right stays `1.2k / 200k`; hover does not show Grok’s bar+`%`.

**Root cause:** `render_footer` cleared `context_hit` then hit-tested that rect → always false during paint.

**Fix:** Sticky `HitArea.hovered` (see pack entry above).

---

### 2026-08-04 — TUI dies right after first frame (`handle_event=false`)

**Symptom:** Full-screen TUI flashed open then closed in ~0.5s. Clean exit (`ok: true`). No panic under `crash/`.

**JSONL:**
```json
{"msg":"tui.first_frame","ctx":{"h":41,"w":167}}
{"msg":"tui.exit","ctx":{"reason":"handle_event=false"}}
{"msg":"tui.stopped","ctx":{"ok":true}}
```

**Root cause:** Context-meter hover tracking returned `hover_changed` from `MouseEventKind::Moved`. After `EnableMouseCapture`, the first mouse-move with no hover flip returned **`false`**, which the run loop treats as quit.

**Fix:** Always return `true` from mouse-move handling; only update `mouse_pos` for the next paint. *(Follow-up 2026-08-05: also `mark_dirty` on hover enter/leave — paint is gated.)*

**Prevention:** Rule 1 above. If adding any `return false` in input handlers, grep for `handle_event=false` and confirm you mean **exit the process**.

**Related:** v1/models work was blamed first; catalog was fine (`context_length ok`). Always trust `tui.exit` reason over gut feel.

---

### 2026-08-04 — Instant exit / plain mode when `stdout_tty=false`

**Symptom:** `no interactive terminal detected (stdin_tty=true stdout_tty=false)` then plain REPL; or raw-mode ENXIO when forcing TUI.

**Root cause:** TUI gate required both stdin and stdout to be TTYs. Wrappers capture stdout.

**Fix:** `tui_available()` / `open_tui_writer()` prefer `/dev/tty`; SIGPIPE ignored at startup.

---

### 2026-08-09 — new crate must register in the three budget JSONs

**Symptom:** CI `budgets` job fails after adding `crates/auth`:
`error: crate 'auth' has no entry in panic_budget.json` (then the same for
`dependency_boundaries.json`, `swallowed_error_budget.json` — each fix
revealed the next).

**Root cause:** `scripts/check_panic_budget.py`,
`check_dependency_boundaries.py`, and `check_swallowed_error_budget.py`
enumerate `crates/*` and require an entry per crate. A new workspace member
fails all three until registered. The boundary file additionally requires
every new dependency *edge* (`cli → auth`, `tui → auth`).

**Fix:** `"auth": 0` in panic budget, `"auth": 6` in swallowed-error budget
(counted via the script's patterns; best-effort stdout flush / browser-page
writes), `"auth": []` + cli/tui edges in dependency boundaries.

**Prevention:** When adding a crate: (1) add it to all three
`scripts/*.json` budgets in the same commit, (2) run the three check
scripts locally before push. They run in seconds and are the whole
`budgets` CI job.

---


### 2026-08-12 — workspace file index (`whycodes-index`): picker + tool fast paths

**Context:** The TUI had a dead `ui/autocomplete.rs` (never wired, sync
`read_dir` per keystroke) and three drifted hand-rolled walkers
(`tools/file/paths.rs`, `memory/code_index.rs`, autocomplete). Researched
Grok Build (`xai-grok-workspace/file_system/`: `ignore`-crate walk → streaming
`nucleo` injection, interned FileIndex, fsnotify deltas) and jcode
(`jcode-fuzzy` typo-tolerant matcher, agentgrep's `ignore`+`globset` walk).

**What shipped:** `crates/index` — background parallel `ignore` walk
(gitignore-aware, `require_git(false)`, symlink-confined, policy-pruned via
the single `policy` module) feeding both a per-root `nucleo` engine (picker)
and a store (tools). `notify` watcher → 250 ms debounced deltas; removals
rebuild that root's engine (nucleo has no per-item removal). `@` in the
prompt (or Ctrl+Space) opens the picker; dirs drill down on Tab.

**Pitfalls encoded in the design:**

- nucleo **0.5.0 (crates.io) has no `snapshot.matches()`** — Grok pins a git
  rev. Use `matched_items()` + per-item `pattern.indices(..)` for score and
  highlight positions (cheap at picker sizes).
- Injector pushes are thread-safe and stream during the walk; `restart()`
  is the only way to remove items.
- Lock order is **fuzzy → store**, never reversed (delta application takes
  both); queries lock only fuzzy, browse/tools lock only store.
- The scanner thread must NOT hold an `Arc` cycle: split `WorkspaceIndex`
  (handle, owns `JoinHandle`) from `Shared` (thread's `Arc` target), or the
  last external drop never runs.
- Hidden files (`.env`!) are excluded by policy — secret hygiene. Tools'
  dotfile-targeting patterns (`.*`) bypass the index and walk the disk.
- `ignore` crate: `filter_entry` drives descent control in both serial and
  parallel mode; `hidden(false)` + own policy so whitelisted dot-dirs
  (`.github`, `.whycodes`) survive.

**Round 2 (async query model) — measured at 34.8k entries (cargo registry):**

- `nucleo.tick(10)` per keystroke blocks 3–16 ms and *returns a stale/empty
  snapshot on timeout* → picker flashes "no matches" while typing. The fix is
  Grok's daemon insight: `reparse + tick(0)` (never blocks), worker threads
  publish, the nucleo `notify` callback flips a shared `AtomicBool`, and the
  run loop adopts results via `poll_matches()` at the loop top.
- Poll-cadence race: when the picker is awaiting results, the idle 500 ms
  `event::poll` must drop to ~16 ms (`awaiting_matches()`), and a
  `results_pending` latch must survive until the first publish — otherwise a
  late notify waits out a long poll.
- Matcher threads scale with cores (2 → 4 ≥9 cores); `query_warm` bench
  38 µs → 17 µs after scaling.
- Measured (release): 34.8k-file cold scan **246 ms**, `query_now` **16 µs**
  (never blocks), settle 6–16 ms (~1 frame), whole-example RSS **~22 MB**
  (~0.6 KB/entry — nucleo stores Utf32 columns; the 200k-entry cap implies
  ~120 MB. If that ever bites, the escape hatch is a custom matcher over the
  store, jcode-fuzzy style — nucleo is the RAM cost, not the store).

### 2026-08-14 — Stream usage is a snapshot, not a delta

**Symptom:** `/cost` and `generate --format json` usage can be 2–N× the
provider's own `usage` object when a gateway repeats `include_usage` on
every chunk. Anthropic streaming can also show `output_tokens = 0` because
official SSE puts `usage` next to `delta`, not inside it.

**JSONL / crash:** `turn.step` `input_tokens` / `output_tokens` disagree
with the last raw `usage` in `WHYCODES_USAGE_DUMP`.

**Root cause:** The agent used `+=` on every `StreamEvent::Usage`. That is
correct for Anthropic's *split* input-then-output events only if each field
is sent once. OpenAI-compat `stream_options.include_usage` is a full
snapshot; some proxies emit it on every token. Summing snapshots inflates
the meter. Anthropic `message_delta` usage was read from `delta.usage`.

**Fix:** `Usage::absorb_stream` (`max` per field) inside one stream step;
`Usage::add` still sums distinct steps (session / subagent). Anthropic
reads `event.usage` first, then nested `delta.usage`. Live check:
`scripts/reconcile_token_usage.py`.

**Prevention:** Do not `+=` stream usage. New provider parsers must dump
the raw object (`usage_dump`) and emit snapshots; tests cover repeated
full snapshots and the Anthropic sibling shape.

---

### 2026-08-16 — `index::tests::watcher_picks_up_changes` flakes under instrumentation / parallel load

**Symptom:** Full workspace or `cargo llvm-cov` runs occasionally fail
`tests::watcher_picks_up_changes` with `create must be indexed` (5 vs 6
files). The test polls with a 15 s deadline, so this is not a slow machine.

**Root cause:** notify-watcher timing under a heavily loaded instrumented
build: the create delta can be delivered after the final settle poll.
Pre-existing — unrelated to coverage-gating work; passes in isolation and
in a second full run.

**Fix:** The CI coverage job runs it with `-- --skip tests::watcher_picks_up_changes`;
the normal `test` job still runs it. Do not try to make the coverage job
reliable by editing the test's deadline — it is genuinely racy.

**Prevention:** When a full `cargo test --workspace` or `llvm-cov` run
reports this failure, re-run in isolation before suspecting the change
under test.

---

### 2026-08-16 — `tui::ui::file_suggest::picker_flow_over_real_index` flakes under parallel load

**Symptom:** Full workspace `cargo test` on the self-hosted runner fails
`picker_flow_over_real_index` after the poll deadline (5 s, then 30 s).
421 other TUI tests pass. Isolated re-run is green.

**Root cause:** Not scan lag. nucleo only publishes a snapshot on `tick`,
and its worker calls `notify` only when `should_notify` is true. `tick`
clears that flag at entry; `tick(0)` then often fails `try_lock` under
load and relies on the worker to flip dirty. If the worker finishes in
that window with `should_notify == false`, notify is skipped. The picker
used to `return` from `poll_matches` unless dirty was already set, so
it never ticked again and sat empty until the deadline.

**Fix:** `poll_matches` always `matching()`-nudges while a rematch is
pending, and does a last-chance `read_matches` once the engine settles
even if dirty stayed false. The test asserts the store actually contains
`src/main.rs` after `wait_ready` so a walk/ignore miss is not mistaken
for this race.

Do **not** clear `results_pending` on the first empty settle. Nucleo can
report `running=false` before workers start; a last-chance read of that
empty snapshot used to drop the pending flag, after which later ticks
never adopted the snapshot (`matches=[]`, `status=Ready { total: 4 }`).
Clear pending only after `matching()` has been seen true (`rematch_seen`)
or the list is non-empty.

**Prevention:** Do not "fix" this by raising the poll deadline again.
If it fails, print `matches` + `scan_status` and check whether the
store has the file (walk) vs the fuzzy snapshot never published (tick).

`set_query` is a no-op on the same pattern, so a missed notify used to
leave the picker empty forever. Two follow-ups:

1. Do not skip `read_matches` while `running && matches.is_empty()` —
   that is the CI failure shape (`Ready { total: 4 }` + `matches=[]`).
2. `poll_matches` calls `WorkspaceIndex::rearm_fuzzy` (reparse +
   `tick(5)`) when the list is still empty.

The coverage job also `--skip`s `picker_flow_over_real_index` the same
way it skips the notify watcher flake; the normal Test job still runs it.

---

### 2026-08-17 — `format::highlight` stream-vs-batch tests flake under parallel load

**Symptom:** `cargo test --workspace` fails
`highlight::tests::stream_append_matches_batch` (and sometimes
`stream_non_prefix_edit_rebuilds`) with token RGB mismatch. Isolated
re-run is green. Colours look like Grok Night vs Grok Day on the same
source.

**JSONL / crash:** none.

**Root cause:** `set_syntax_theme` is process-wide. The stream highlighter
commits line colours under theme A; a sibling test
(`switching_syntax_theme_recolours_rust`) flips the theme to Day; the
batch `highlight_uncached` then paints theme B. The assertion compares
the two.

**Fix:** Those tests share a `lock_theme()` mutex and pin Grok Night
before comparing stream vs batch.

**Prevention:** Any new test that calls `set_syntax_theme` or compares
stream colours to `highlight_uncached` must take the same lock. Do not
"fix" by relaxing the colour assertion.

---

### 2026-08-17 — sandbox tests panic when the runner has no `bwrap`

**Symptom:** CI `Test (linux)` and `Coverage (line floor)` fail in
`whycodes-sandbox`: `bwrap is required for this test`, or
`prepare_delegates_to_prepare_with_real_availability` asserts
`Bubblewrap` while the host backend is `Host`.

**JSONL / crash:** none.

**Root cause:** The 100% coverage lift assumed the self-hosted runner has
bubblewrap. It does not (`backend_available()` is false). Tests used
`assert!(backend_available())` instead of skipping, and one test
hard-coded `Backend::Bubblewrap`. Installing `bwrap` on the runner is
not a safe default: a container without user namespaces would then
*find* the binary and fail later on real `bwrap` exec.

**Fix:** Prepare-only tests drive `prepare_bwrap_bin(Some(stub))` and do
not need a real binary. Live `run()` isolation tests live in
`crates/sandbox/tests/live.rs`. Unit tests live in `src/tests.rs`; the
100% crate floor ignores that file so a missing `bwrap` cannot leave
host-only assertion arms as missed lines.

**Prevention:** Keep host-dependent sandbox tests out of production
`.rs` files counted by `--fail-under-lines 100`. CI ignores
`tests.rs` on those crates and prints `--show-missing-lines`.

---

### 2026-08-20 — sandbox 100% floor misses `SSH_AUTH_SOCK` unset

**Symptom:** `Coverage (line floor)` fails `whycodes-sandbox` with
`Uncovered Lines: crates/sandbox/src/bwrap.rs: 105` (the closing `}` of
`if let Ok(auth_sock) = std::env::var("SSH_AUTH_SOCK")`). Workspace 78%
and the other 100% crates are green. Isolated `cargo test -p whycodes-sandbox`
is green.

**JSONL / crash:** none.

**Root cause:** That `if let` sits inside `if let Some(home) = HOME`.
Tests either unset `HOME` (skip the whole block) or set `SSH_AUTH_SOCK`
to a path. The self-hosted runner usually already has an agent socket, so
the `Err` arm is never taken and llvm-cov attributes the closing brace
to the miss.

**Fix:** `prepare_bwrap_without_home_and_root_auth_sock` now also sets
`HOME` and *removes* `SSH_AUTH_SOCK`.

**Prevention:** Env-dependent `if let Ok(std::env::var(…))` in a 100%
crate needs an explicit unset test under `env_lock()`, not just the
happy path. Do not "fix" by dropping the crate off the 100% list.

---


### Template (copy for new entries)

```markdown
### YYYY-MM-DD — short title

**Symptom:** What the user saw.

**JSONL / crash:** Key `msg` lines or “none”.

**Root cause:** One paragraph.

**Fix:** What changed (files / behavior).

**Prevention:** Rule or checklist item so we do not reintroduce it.
```

---

## Related paths

| Area | Path |
|------|------|
| Event loop | `crates/tui/src/run.rs` |
| Keys / mouse | `crates/tui/src/input.rs` |
| Manual emulator matrix | `docs/tui-term-matrix.md`, `scripts/tui_term_matrix.sh` |
| Context meter footer | `crates/tui/src/ui/status.rs` |
| Model catalog / context_length | `crates/llm/src/model_catalog.rs`, `capabilities.rs` |
| Logging / JSONL / crash | `crates/core/src/logging.rs` |
| CLI entry / SIGPIPE / TUI gate | `crates/cli/src/main.rs` |
| Agent rules (build) | `AGENTS.md` |
| CI budgets (panic / swallow / edges) | `scripts/check_*.py`, `scripts/*_budget.json`, `scripts/dependency_boundaries.json` |
| CI workflow | `.github/workflows/ci.yml` |

## Axum: `/s/{id}` vs `/s/{id}.json` route conflict (2026-08-13)

**Symptom:** `whycodes serve` panics at router build:
`Invalid route "/s/{id}.json": insertion failed due to conflict with previously registered route: /s/{id}`.

**Cause:** axum/matchit treats `{id}` as capturing the rest of the segment; a second
static-suffix route on the same path pattern is rejected. (axum 0.8 renamed `:id` → `{id}`.)

**Fix:** one route `/s/{id}` and dispatch on whether `id` ends with `.json` / `.md`
(`share_dispatch` in `crates/server/src/routes.rs`).


## Antigravity OAuth: missing scopes look like Gemini Code Assist sunset

**Date:** 2026-08-24 · **Area:** `crates/auth/src/providers.rs`, `crates/auth/src/cca.rs`

**Symptom:** `google-antigravity` browser sign-in succeeds, then fails with
`Google Code Assist: This client is no longer supported for Gemini Code Assist`.

**Cause:** Native Antigravity (and oh-my-pi's working client) request
`cclog` + `experimentsandconfigs` in addition to `cloud-platform` / userinfo,
identify as `antigravity/hub/2.8.0 (aidev_client; os_type=darwin; arch=arm64;
cl=963137146)` regardless of host OS, and poll `:onboardUser` LROs with GET.
Without the extra scopes or with a Linux/x86_64 User-Agent,
`loadCodeAssist` classifies the session as Gemini Code Assist for individuals
(sunset 2026-06-18) and puts `free-tier` in `ineligibleTiers`. A second trap:
that ineligibility is expected even for a valid Antigravity account that still
has `standard-tier` (or another paid tier) in `allowedTiers` — aborting on
the free-tier reason skips the working onboard path.

**Fix:** request the native five-scope set; pin the darwin/arm64 User-Agent;
poll operations with GET; only surface free-tier ineligibility when
`allowedTiers` is empty.

## Code Assist onboarding: LRO polling and response shapes

**Date:** 2026-08-24 · **Area:** `crates/auth/src/cca.rs` (google-antigravity OAuth)

Two silent-failure traps when onboarding a fresh Google token via
`:onboardUser` (`cloudcode-pa.googleapis.com/v1internal`):

1. **Tier choice.** Pro/paid accounts report only the
   `userDefinedCloudaicompanionProject: true` tier (`standard-tier`) in
   `loadCodeAssist.allowedTiers`. Forcing `free-tier` returns **403 Forbidden**
   ("not eligible for individuals"). Pick the user-defined-project tier first.
2. **LRO result & project resolution.** The operation can take minutes;
   polling for ~10 s then reading `response.cloudaicompanionProject.id` fails
   with "no project id". Two subtleties confirmed by diffing oh-my-pi's proven
   implementation (`packages/ai/src/registry/oauth/google-antigravity.ts`):
   accounts whose `loadCodeAssist` response carries a `currentTier` are ALREADY
   onboarded — never call `onboardUser` for them; and the authoritative source
   of the project is a **fresh** `:loadCodeAssist` call after provisioning,
   where `cloudaicompanionProject` ships as a bare **string** (the LRO body
   frequently carries no project at all). Also surface
   `ineligibleTiers[].reasonMessage` before attempting free-tier onboarding —
   it is Google's own explanation for the 403.

Debugged 2026-08-24 after real-world 403 → "did not yield a project id" reports.
