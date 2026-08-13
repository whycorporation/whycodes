# Plan — Claude Code research follow-ups (A1–A7)

**Status:** **shipped** (2026-08-07) · **Priority:** terminal product polish  
**Source:** internal Claude Code research report §10A  
**Related:** [FEATURES.md](FEATURES.md), [status.md](status.md)  
**Policy:** Ideas only — re-implement in Rust; do **not** copy proprietary code or prompts.

## Goal

Convert the research report’s high-ROI opportunities (**A1–A7**) into shippable whycode work, ordered by value vs cost, without turning into “Claude Code parity.”

Success = each row’s acceptance criteria green, FEATURES.md cells updated, no copyright risk.

## Non-goals

- IDE bridge / ACP (already **deferred** — B1).
- Persistent cron, marketplace, voice, computer-use, autoDream (B/C).
- Bash AST / yolo ML classifier port.
- Copying prompts or source from proprietary implementations.

## Order (recommended)

| Order | ID | Title | Score | Effort | Depends on |
|------:|----|-------|------:|--------|------------|
| 1 | **A1** | PromptCommand workflows (`/review`, `/commit`, …) | 5 | S–M | — |
| 2 | **A4** | `/context` visualization | 4 | S | — |
| 3 | **A2** | LLM-backed compact | 4 | M | provider + compact threshold |
| 4 | **A3** | Path permission globs | 4 | M | PermissionSet |
| 5 | **A5** | MCP server export | 4 | M–L | mcp crate |
| 6 | **A6** | Multimodal file read | 3 | M | provider vision support |
| 7 | **A7** | Idle prompt suggestions | 3 | S–M | optional small model |

Ship **A1 → A4 first** (no deep API redesign). Then A2/A3. A5 when agent-to-agent matters. A6/A7 last (nice-to-have).

---

## A1 — PromptCommand workflows

**Why:** Claude Code’s “magic” is often slash → fixed prompt + tool allowlist, not more tools.  
**whycode today:** `/init` injects a prompt; `.whycode/commands/*.md` + OpenCode-style commands exist but few built-ins.

### Design

1. Built-in **prompt slash commands** (or ship as bundled markdown under a known path):
   - `/review` — staged + unstaged review; prefer `git_diff`, `git_status`, `read`, `grep` (no write unless user asks).
   - `/security-review` — secrets, injection, dangerous shell patterns, authz.
   - `/commit` — stage-aware message draft + optional `git_commit` only after user confirm (or plan-mode default).
2. Optional later: `/commit-push-pr` behind `gh` availability.
3. Each command defines:
   - `name`, `hint` (slash suggest)
   - `prompt_template` (user message body)
   - optional `tool_profile` override or soft allow-hint in prompt
   - optional `agent` (`plan` for review)

### Tasks

- [ ] Inventory existing custom-command loader (`Config::load_command_files`) — reuse vs hardcode.
- [ ] Add built-in definitions (Rust const or `crates/cli` / `crates/tui` shared registry).
- [ ] Wire TUI + plain CLI slash handlers to enqueue `pending_prompt` (same as `/init`).
- [ ] Register in `BUILTIN_SLASH_COMMANDS` + help.
- [ ] Smoke: `/review` with dirty tree produces a useful first turn without write tools if agent=plan.

### Acceptance

- `/review`, `/security-review`, `/commit` appear in slash menu and work in TUI + `--plain`.
- No new tools required; uses existing git/file tools.
- FEATURES “Custom slash / commands” note or workflow row updated if needed.

### Config sketch (optional)

```toml
[commands.review]
# overrides built-in if set
```

---

## A2 — LLM-backed compact

**Why:** Char drop + prune loses nuance on long sessions; OpenCode/CC use summary agents.  
**whycode today:** `Session::compact` drop + prune + circuit breaker; no LLM summary.

### Design

1. When `token_count > compaction_threshold` **and** circuit breaker not tripped:
   - First: existing prune/truncate (cheap).
   - If still over: call **small/fast model** (`session.model_fast` or title model chain) with a fixed “summarize dropped prefix” prompt.
   - Replace dropped prefix with one user/system summary message (same shape as today’s stub summary, richer content).
2. Budget: max summary tokens ~2–4k; timeout; on failure fall back to char compact and count breaker failure.
3. Config: `session.compaction_llm = "auto" | "off"` (default `auto` when API key present).

### Tasks

- [ ] `CompactOutcome` already exists — add `summary_source: Drop | Llm`.
- [ ] Implement `session.compact_with_summary(...)` or agent-side helper using provider registry.
- [ ] Never block first TTFT of a *new* user turn longer than N seconds; prefer background only if safe (v1: sync before LLM step is OK if model is mini).
- [ ] Unit test: mock provider returns summary; messages shrink and include summary text.
- [ ] Document in `/info` when last compact used LLM.

### Acceptance

- Long synthetic session (>threshold) produces a non-empty semantic summary line, not only “trimmed N messages”.
- Failure path does not loop (breaker still works).
- `compaction_llm = "off"` restores pure local compact.

---

## A3 — Path permission globs

**Why:** Shell already has `bash(git *)`; file tools still tool-name-only.  
**CC idea:** `FileEdit(/src/*)`, `FileRead(*)`.

### Design

1. Extend rule language:
   - `read(src/**)`, `edit(crates/**)`, `write(**/*.md)`, `apply_patch(…)`
   - Glob: `**`, `*`, trailing path prefixes; resolve relative to project root.
2. Evaluation order in `execute_with_permission` for file tools:
   - risk N/A → path rule Deny → deny
   - path rule Ask → prompt with path
   - path rule Allow → skip tool-level Ask if tool is Ask
3. Dangerous patterns: allow `write(**)` should not silently allow absolute `/etc/**` (keep outside-project as Ask/Deny via existing path policy).

### Tasks

- [ ] Parse `tool(glob)` in `PermissionSet` (mirror `action_for_shell`).
- [ ] `action_for_path(tool, path) -> Option<PermissionAction>`.
- [ ] Wire into write/edit/apply_patch/read (read optional — default Allow).
- [ ] Tests: allow `edit(src/*)` for `src/a.rs`, deny `edit(src/*)` for `../secret`.
- [ ] Document in README/config sample.

### Acceptance

```toml
[permission]
"edit(src/**)" = "allow"
"write(**)" = "ask"
"bash(git *)" = "allow"
```

behaves as documented in TUI permission prompts.

---

## A4 — `/context` visualization

**Why:** Power users need to see *what* fills the window (CC `/context`).  
**whycode today:** `/info` session fields + context %; no breakdown.

### Design

`/context` system message (and plain CLI) showing:

| Block | Content |
|-------|---------|
| Budget | used / max / % (provider usage if present) |
| Messages | count by role; largest N tool results (name + chars) |
| Tools | profile + activated deferred (`tool_search`) |
| Memory | enabled + last inject chars (if any) |
| Compact | threshold, last outcome, breaker paused? |
| Cwd | project + worktree override |

No new LLM call. Pure local introspection.

### Tasks

- [ ] Helper `context_report(session, agent, config, app)`.
- [ ] Slash `/context` in TUI + plain + slash suggest.
- [ ] Cap list lengths so report stays short.

### Acceptance

- `/context` readable in one screenful for a medium session.
- Does not require API key.

---

## A5 — MCP server export

**Why:** Codex/others expose agent tools as MCP server; FEATURES gap.  
**whycode today:** MCP **client** only.

### Design

1. CLI: `whycode mcp-serve` (stdio MCP).
2. Expose a **curated** tool set (core profile or config allowlist) — not full github spam by default.
3. Map whycode `Tool` → MCP tool list/call; working_dir = cwd; permissions = env auto-approve or restrictive default.
4. Auth: none for local stdio; document danger of full-auto.

### Tasks

- [ ] Evaluate `rmcp` / existing `whycode-mcp` for server role.
- [ ] stdio server loop + tool dispatch via `ToolExecutor`.
- [ ] Config: `mcp_server.tools = "core" | "full" | [names…]`.
- [ ] Integration test with mock client or unit dispatch.
- [ ] FEATURES: MCP as server → ✅.

### Acceptance

- Claude Desktop / Cursor can add `whycode mcp-serve` and call `read`/`grep` successfully.
- Default is not “silent shell full access”.

---

## A6 — Multimodal file read

**Why:** CC FileRead handles images/PDFs; whycode is path-attach partial.  
**Depends:** provider vision (Anthropic/OpenAI/Google).

### Design

1. `read` tool: if path is image (`png/jpg/webp/gif`) or PDF (first page or text extract):
   - Image → content block for next model turn **or** tool result with base64 + media type when provider supports.
   - PDF → text extract (prefer) or page image.
2. Cap size (e.g. 4MB); refuse huge binaries.
3. Feature flag / config `tools.read_images = true`.

### Tasks

- [ ] Detect mime by extension + magic bytes.
- [ ] Wire `ContentBlock::Image` into tool results path (agent → session messages).
- [ ] Provider adapters already multimodal? verify anthropic/openai/google.
- [ ] Tests with small fixture PNG.

### Acceptance

- “What’s in screenshot.png?” with `read` yields non-error model path when vision model selected.
- Non-vision models get a clear text fallback (“image attached but model may not support vision”).

---

## A7 — Idle prompt suggestions

**Why:** CC PromptSuggestion / speculation — “alive” UX.  
**Careful:** cost + noise; default off or rare.

### Design

1. After turn completes (idle), optionally request **1 short** follow-up suggestion from `model_fast` (cache-friendly, tiny max_tokens).
2. TUI: ghost text or toast “Tab to accept suggestion”; Esc dismiss.
3. Config: `tui.prompt_suggestions = "off" | "idle"` (default **off** until polished).
4. No filesystem speculation (CC speculation copies files — out of scope).

### Tasks

- [ ] Gate + config.
- [ ] Async suggest after `TurnOutcome::Ok` when idle.
- [ ] Tab accepts into input buffer.
- [ ] Cancel in-flight on new keystroke.

### Acceptance

- Default off: zero extra API calls.
- When on: at most one suggest per idle window; never blocks agent_busy.

---

## Tracking

| ID | Status | Notes |
|----|--------|-------|
| A1 | **done** | `/review`, `/security-review`, `/commit` built-in PromptCommands |
| A2 | **done** | `session.compaction_llm` + small-model summary after local compact |
| A3 | **done** | `edit(src/**)` path rules + agent gate |
| A4 | **done** | `/context` TUI + plain |
| A5 | **done** | `whycode mcp serve [--tools core\|full]` stdio MCP server |
| A6 | **done** | `read` images → WHYCODE_IMAGE_B64 → session Image block |
| A7 | **done** | `tui.prompt_suggestions = "idle"`; Tab accepts; default off |

Update this table + [status.md](status.md) when a row ships. Prefer **one PR per ID** (or A1+A4 together as “slash UX”).

## Verification (per PR)

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p whycode-agent -p whycode-tools -p whycode-tui -p whycode-core
# A5 additionally:
# cargo test -p whycode-mcp
```

## Decision log

| Date | Decision |
|------|----------|
| 2026-08-07 | A1–A7 promoted from research report into this plan; order A1,A4,A2,A3,A5,A6,A7. |
| 2026-08-07 | A7 default **off**. A5 default restrictive tool set. No CC source vendoring. |
