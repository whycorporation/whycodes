# Whycode know-how

Living notes for bugs that are easy to reintroduce — especially TUI, terminal I/O, and silent exits. **Read this before changing the event loop, mouse handling, or terminal setup.**

When you fix a non-obvious bug: **append an entry** (newest first under [Log](#log)). Keep each entry short: symptom → root cause → fix → prevention.

---

## Quick diagnosis

| Symptom | First check |
|--------|-------------|
| TUI opens and dies immediately | `tail -40 ~/.local/share/whycode/logs/unified.jsonl` |
| Panic? | `ls ~/.local/share/whycode/crash/` (empty ⇒ usually not a panic) |
| Silent clean exit | Look for `tui.exit` / `tui.loop_error` / `main.exit_error` in JSONL |
| No TUI, plain mode | `stdin_tty` / `stdout_tty` / `/dev/tty` in `tui.starting` |

Lifecycle events written to **`~/.local/share/whycode/logs/unified.jsonl`** (always-on):

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

```bash
# After a bad run:
tail -40 ~/.local/share/whycode/logs/unified.jsonl
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

### 5. `/v1/models` context window

- Config-driven only: `base_url` / `api_key` / headers from the active provider — **no hard-coded gateway hosts**.
- TUI must **not** store the full gateway catalog (thousands of models). Keep a single `api_context_window` for the active model.
- Failures are non-fatal; meter falls back to built-in / `session.max_context_tokens`.
- Opt-out: `WHYCODE_NO_MODEL_CATALOG=1`.

### 6. `max_tokens` vs `context_window`

| Field | Meaning |
|-------|---------|
| `ModelConfig.max_tokens` | Completion cap sent to the API |
| `ModelConfig.context_window` / API `context_length` | Full prompt+completion budget (meter, compact) |

Do not use rate-limit headers (`x-ratelimit-limit-tokens`) as context window — those are TPM quotas.

### 7. Build before “done”

See root `AGENTS.md`. After Rust edits: `cargo check` / `cargo build -p whycode-cli` (and tests when logic changes).  
Users often run **`./target/release/whycode`** — rebuild **release** when verifying TUI fixes they will run that way.

---

## Log

### 2026-08-06 — Intent: question vs change vs plan

**Symptom:** In build mode the model often edits when the user only asked a question, or skips planning on large design asks.

**Root cause:** No product-level intent layer — only free-form model judgment. Competitors use hard modes (Cursor Ask/Plan/Agent, OpenCode Build/Plan) plus prompt rules, not a silent ML router.

**Fix:** Three layers: (1) primary agents `build` / `plan` / `ask` with tool denylists; (2) prompt protocol in `prompts/{build,plan,ask}.txt`; (3) zero-LLM heuristic (`crates/agent/src/intent.rs`) injects ephemeral `<whycode_intent>` on the **request** only (not session history) so system prompt cache stays stable. Config: `session.intent_guidance = auto|off|always`.

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

**Symptom:** `whycode --version` (Boot floor in comparison tables) paid multi-ms for a multi-thread Tokio runtime that never ran work. Windows baseline ~21 ms was mostly binary page-in + that setup.

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

**Not covered:** Raw clipboard bitmap paste (no path) — host-dependent; path drop is the portable path.

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

**Roadmap:** [plan-latency-competitors.md](plan-latency-competitors.md)

**Prevention:** Do not drop latest-user cache breakpoint; do not reintroduce sequential-only tool loops or full tool dump as default.

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

**Repro (outside whycode):** hang/slow `/v1/models` + concurrent `/v1/chat/completions` → chat TTFT balloons; alone chat stays ~2–3s.

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
| Context meter footer | `crates/tui/src/ui/status.rs` |
| Model catalog / context_length | `crates/llm/src/model_catalog.rs`, `capabilities.rs` |
| Logging / JSONL / crash | `crates/core/src/logging.rs` |
| CLI entry / SIGPIPE / TUI gate | `crates/cli/src/main.rs` |
| Agent rules (build) | `AGENTS.md` |
