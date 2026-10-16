# Prompt cache

> Status: Implemented · Updated: 2026-10-16

"Prompt cache" in this codebase means the provider-side reuse of an already
processed request prefix. Only the Anthropic Messages provider gets explicit
`cache_control` breakpoints; every other provider is sent a plain request and
relies on whatever implicit prefix caching it offers. The agent's job is to
keep the prefix byte-stable across the steps of a turn and across turns:
message-independent text goes in the system prompt, per-turn material rides on
the last user message. Everything below is read from the code; unconfirmed
details say "not verified".

## How it works

### 1. Per provider family

| Family | Explicit markers | Cache usage reported | Where |
|---|---|---|---|
| Anthropic Messages (`anthropic` provider, `crates/llm/src/providers/anthropic.rs`) | yes, when `request.use_prompt_cache` (`anthropic.rs:311-316`) | `cache_creation_input_tokens` / `cache_read_input_tokens` as `StreamEvent::CacheUsage` | `crates/llm/src/cache.rs` |
| OpenAI-compatible chat (`openai`, `openrouter`, `groq`, `together`, `deepseek`, `mistral`, `xai`, `custom`, `copilot`), Codex Responses | none; no `prompt_cache_key`/`store`-style fields are sent (grep) | none: `cached_tokens` is deliberately left unset (`crates/llm/src/openai_compat.rs:649-660`, `crates/llm/src/providers/codex.rs:274-277`) | implicit prefix caching only |
| Google / Code Assist / Antigravity, Ollama | none | none (`google.rs:113-114`, `codeassist.rs:807-808`, `ollama.rs:327-328`) | implicit only |

`LlmRequest::use_prompt_cache` is consumed by exactly one site,
`anthropic.rs:312` (grep over `crates/llm/src`).

### 2. Anthropic breakpoints (`apply_anthropic_cache_policy`)

`crates/llm/src/cache.rs:103-190`, with `CacheConfig::default()`
(tools + system + 1 message); at most 4 markers:

```
tools[ last ]              cache_control: ephemeral      (cache.rs:110-120)
system  (string -> [ {text, cache_control} ])            (cache.rs:122-145)
messages[ last role=user ] last text part (or last part)  (cache.rs:147-190)
```

The wire order is `tools`, `system`, `messages`; a byte change in tools
invalidates everything after it. `Role::Tool` messages become wire role
`user` (`anthropic.rs:324-331`), so "latest user message" on the wire is
usually the latest tool-result message mid-turn: the third breakpoint moves
forward with every step, which is the intended "cache up to here" marker.
`CachePolicy::None` exists (`cache.rs:21-35`) and maps from
`session.prompt_cache = "none"`.

### 3. What the request is made of, in order

`Session::build_request` (`crates/session/src/session.rs:928-950`) copies
`system_prompt`, `messages` (after `pairing::paired_messages` repair) and the
step's tool definitions, with `use_prompt_cache: true`. The agent then
overrides the flag (`turn.rs:277`), appends per-turn suffixes, and attaches
thinking parameters.

**System prompt** — rebuilt by `refresh_session_memory` before each
non-trivial user turn (TUI `crates/tui/src/run/persist.rs:284-314` via
`prepare_turn`, `crates/tui/src/run/mod.rs:4418-4428`; REPL
`crates/cli/src/cmd/helpers.rs:541-571`). Layers, top to bottom:

1. Role prompt: `AgentInfo.system_prompt` or `crates/agent/prompts/<agent>.txt`
   (`Agent::system_prompt_for`, `crates/agent/src/agent/mod.rs:1137-1147`).
2. Provider/model overlays, each as `# <label>` (`with_prompt_overlays`,
   `mod.rs:1109-1127`; `[system_prompt_overlays]` config).
3. `# Project Instructions (…)` + `# Additional instructions (…)`:
   `AGENTS.md` and sibling instruction files from the project dir up to the
   git root (`crates/agent/src/context_files.rs:20-29`, `154-168`, `172-184`).
4. `# Skills` catalog, names + descriptions (`crates/skill/src/registry.rs:99-108`).
5. `# Custom agents` catalog (`crates/agent/src/custom_agents.rs:157-163`).
6. `# Environment`: `Today's date`, platform, shell, working directory
   (`with_runtime_context_in`, `mod.rs:1159-1181`).
7. `# Auto Memory`: the capped `MEMORY.md` index, query-independent
   (`with_project_memory(..., None)`, `helpers.rs:566`;
   `crates/memory/src/service.rs:494-512`).

Nothing query-dependent is in the system prompt: recalled facts, code RAG and
`# Past sessions` are built by `recall_turn_context` and stored in
`session.turn_context` (`service.rs:689-725`; `helpers.rs:567`).

**Tools** — `Agent::step_tools` (`turn.rs:22-34`): the executor's memoized
definitions for (permission fingerprint, profile, activated extras), sorted by
name, minus `swarm` when disabled. Trivial first prompts send no tools at all
(`tools_free_chat`, `turn.rs:120-122`, `189-193`). MCP tools are registered
as ordinary tools (`crates/agent/src/mcp_load.rs:134-139`) and, under the
default `core` profile, are deferred like every non-core tool
(`crates/tools/src/profile.rs:53-67`); they enter the array only after
`tool_search select` or with `tool_profile = "full"`. There is no deferred-tool
list in the system prompt: the catalog is returned by `tool_search list`
(`crates/agent/src/agent/dispatch.rs:308-340`); `prompts/build.txt:39` only
mentions `tool_search`.

**Messages** — the session history, then request-time suffixes appended to
the *last `Role::User` message* by `append_request_user_suffix`
(`mod.rs:322-341`), never stored in the session:

- `turn_context` recall block `<recalled-context>…</recalled-context>`,
  every step of the turn (`append_turn_context`, `mod.rs:310-318`, `turn.rs:276`).
- Intent posture `<whycodes_intent confidence=… kind=…>` — step 1 only
  (`turn.rs:291-304`; `crates/agent/src/intent.rs:188-224`).
- Magic keyword notice `<whycodes_keyword name="ultrathink"|"orchestrate">`
  — step 1 only (`turn.rs:305-313`; `crates/agent/src/magic_keywords.rs:12-45`).

Hook feedback (`<hook-context>`) is different: it is written into the stored
user message before the first step (`crates/agent/src/agent/lifecycle.rs:70-87`).

### 4. Cache-miss detection and the status line

Per step the loop folds `StreamEvent::CacheUsage` into `turn_usage`
(`turn.rs:589-594`; `Usage::absorb_stream_cache`, max-fold,
`crates/core/src/types.rs:266-275`). After the stream, `note_cache_read`
(`mod.rs:1387-1399`) stores this step's `cache_read_input_tokens` per session
id and compares with the previous step's; `cache_miss_notice`
(`crates/agent/src/cache_watch.rs:134-145`) emits
`Prompt cache miss: Xk tokens sent uncached (last step read Yk from cache)`
when the previous read was ≥ 2 048, this read is under a quarter of it, and
`input + cache_creation` ≥ 2 048. It is logged and sent as `TurnEvent::Status`
(`turn.rs:690-693`). Because non-Anthropic providers never fill the cache
fields, the notice can only fire on the Anthropic provider.

### 5. `/fresh`

`Agent::skip_prompt_cache_next` sets a one-shot flag (`mod.rs:543-547`);
`run_turn_steps` swaps it off at turn start (`turn.rs:109-111`) and sends
`use_prompt_cache = false` (`turn.rs:277`) — on Anthropic, no `cache_control`
at all for that turn's steps — and also bypasses the response cache
(`turn.rs:353`). The miss notice is suppressed for that turn (`mod.rs:1395-1397`).
Slash handlers: `crates/tui/src/run/slash.rs:366-372`,
`crates/cli/src/cmd/run.rs:988-991`.

### 6. Keep-alive (`cache_warm_minutes`)

`Agent::cache_warm_job` (`crates/agent/src/agent/cache_warm.rs:40-103`)
rebuilds the last step's request (same tools, system prompt, `turn_context`,
thinking settings), truncates the messages to the last non-assistant one,
opens a stream through `LlmTransport::stream` and hangs up on the first event
that is not `Usage`/`CacheUsage`. It returns `None` when prompt caching is
off. `warm_due` (`cache_warm.rs:24-34`) fires every `WARM_EVERY = 270 s`
while the idle time is under the window. Callers: TUI `maybe_warm_cache`
(`crates/tui/src/run/mod.rs:5118-5142`, skipped while a turn runs) and the
daemon `crates/server/src/keep_warm.rs`. Usage and price are folded into the
session (`record_cache_warm`, `cache_warm.rs:106-115`).

### 7. Preconnect

`whycodes_llm::preconnect::spawn` (`crates/llm/src/preconnect.rs:59-75`) does a
bare `GET` of the endpoint origin through the shared client, once per origin
per 90 s, 10 s timeout, draining ≤ 64 KiB so HTTP/1 can pool the socket. The
TUI triggers it on keystrokes when the input is non-empty, not a slash command,
not attached to a daemon and `session.preconnect` is on
(`crates/tui/src/run/mod.rs:2309-2315`, `6198-6201`; `Agent::preconnect`,
`mod.rs:465-469`). It warms the TCP/TLS connection, not the provider cache.

### 8. Tool definitions cache (`defs_cache`)

`ToolExecutor::get_definitions_profile_extra`
(`crates/tools/src/executor.rs:168-203`) memoizes the `Arc<[ToolDefinition]>`
per `DefsCacheKey { perm fingerprint, profile, sorted extra }` so a step does
not rebuild JSON schemas; `register` / `register_as` clear it
(`executor.rs:115-129`). It saves CPU only; the bytes sent are the same
whether or not it hits.

### Audit: dynamic content before the stable prefix

Phase 2 deliverable. "Prefix" = Anthropic wire order `tools → system →
messages[..last user]`. For implicit-caching providers the same order applies
without markers. Each row: what can differ between two consecutive steps or
turns of one session, where it sits, and what it invalidates.

| # | Block | Position | Changes when | Evidence |
|---|---|---|---|---|
| 1 | Tool definitions | before system (first on the wire) | `tool_search select` activates a deferred tool mid-turn → next step's array differs; `load_mcp` replaces the executor; permission set or profile change (`/agent`); `swarm` toggled; first-turn `tools_free_chat` sends `[]` then later turns send tools | `turn.rs:22-34`, `dispatch.rs:350-357`, `mod.rs:1026-1043`, `turn.rs:120-122` |
| 2 | `# Environment` — `Today's date` | inside system | date rollover at local midnight: the prompt is rebuilt before each non-trivial turn and `with_runtime_context_in` formats `chrono::Local::now()` | `mod.rs:1159-1181`, `helpers.rs:563-566` |
| 3 | `# Project Instructions`, `# Skills`, `# Custom agents` | inside system | the files on disk change (disk walk on every rebuild); a nested dir's instructions are **not** here — they are appended to the `read` tool result (`attach_nested_instructions`) precisely to keep this stable | `context_files.rs:20-29`, `turn.rs:1063-1090` |
| 4 | `# Auto Memory` index | end of system | `MEMORY.md` edited (`/remember`, retain); rebuilt each non-trivial turn | `service.rs:494-512`, `helpers.rs:566` |
| 5 | Provider/model overlays, role prompt | top of system | `/models`, `/agent`, model plan routing | `mod.rs:1099-1127`, `helpers.rs:562-563` |
| 6 | Old tool results, in place | middle of messages | `truncate_large_tool_results` / `prune_old_tool_results` every step (first time a body exceeds the cap), `shake_old_tool_results` once over ¾ threshold, `apply_rewind`, compaction | `turn.rs:212-221`, `session.rs:1109-1208`, `turn.rs:863-869` |
| 7 | `<recalled-context>` suffix | appended to the last stored `Role::User` message | same bytes on every step of a turn; but when a *new* user message is appended mid-turn (failed-tool recovery `turn.rs:886-889`, steering `mod.rs:856-865`, background completions `mod.rs:827-845`, stream-rule note `turn.rs:1028-1032`) the suffix migrates to that message, so the earlier prompt's bytes change and the prefix after it is lost | `mod.rs:322-341`, `turn.rs:276` |
| 8 | `<whycodes_intent>` posture, `<whycodes_keyword>` notice | appended to the last user message, step 1 only | present on step 1, absent on step 2+ of the same turn: the user message is re-sent without them, so the step-1 cache entry for that message cannot be read on step 2 (tools + system still hit) | `turn.rs:291-313` |
| 9 | `cache_control` on the latest wire-user message | last message | moves forward every step by design | `cache.rs:147-190` |
| 10 | Compaction carrier | rewrites the whole message list | any compaction | `session.rs:1295-1361` |

Not in the prefix (safe): hook feedback is stored once in the message
(`lifecycle.rs:70-87`); the `Role::Tool` → wire-user conversion is
deterministic; thinking parameters are top-level body fields, not blocks
(`crates/agent/src/thinking_acc.rs` `attach_thinking_request`, not verified
whether every provider ignores them for caching).

Mitigations already in place: trivial prompts keep the previous system prompt
byte-stable and clear `turn_context` (`helpers.rs:548-558`); nested
instructions go to tool results; recall rides on the user message instead of
the system prompt (`service.rs:689-694` quotes the measured cost of the
alternative).

## Configuration and overrides

| knob | where | effect |
|---|---|---|
| `prompt_cache` (`"auto"` default; `none`/`off`/`false`/`0`) | `[session]`, `crates/config/src/types.rs:1347-1349`; parsed `mod.rs:623-630` | `off` → `use_prompt_cache = false` on every request; Anthropic sends no markers; keep-alive disabled |
| `/fresh` | slash command | one turn without markers and without response-cache lookup |
| `cache_warm_minutes` (default 0) | `[session]`, `types.rs:1350-1354` | idle keep-alive window; TUI and daemon |
| `preconnect` (default true) | `[session]`, `types.rs:1382-1385` | background origin dial while typing (TUI only) |
| `tool_profile` (`core` default / `full`) | `[session]`, `types.rs:1343-1346` | size and stability of the tools block |
| `response_cache` (`"auto"` / `off`) | `[session]`, `types.rs:1379-1381` | process-local text replay for tools-free requests (`crates/llm/src/response_cache.rs`); unrelated to the provider cache |
| `[memory] enabled`, `auto_inject` | config | whether `turn_context` is built at all |

No knob changes the breakpoint layout (`CacheConfig::default()` is hard-coded
at `anthropic.rs:313-315`), the 270 s keep-alive cadence, or the miss-notice
thresholds.

## Implementation files

- `crates/llm/src/cache.rs` — `CachePolicy`, `CacheConfig`, `apply_anthropic_cache_policy`.
- `crates/llm/src/providers/anthropic.rs` — `build_body` applies the policy; usage events carry cache counts.
- `crates/llm/src/openai_compat.rs`, `providers/codex.rs` — why `cached_tokens` is not surfaced.
- `crates/session/src/session.rs` — `build_request`, `set_system_prompt`, in-place tool-result rewrites.
- `crates/agent/src/agent/mod.rs` — `append_turn_context`, `append_request_user_suffix`, `note_cache_read`, `skip_prompt_cache_next`, `preconnect`, system prompt layering.
- `crates/agent/src/agent/turn.rs` — flag handling, suffix injection, usage fold, miss notice.
- `crates/agent/src/cache_watch.rs` — `cache_miss_notice`.
- `crates/agent/src/agent/cache_warm.rs`, `crates/server/src/keep_warm.rs`, `crates/tui/src/run/mod.rs` (`maybe_warm_cache`) — keep-alive.
- `crates/llm/src/preconnect.rs` — origin dial.
- `crates/memory/src/service.rs` — `inject_parts`, `recall_turn_context`, `apply_memory_prompt`.
- `crates/agent/src/context_files.rs`, `crates/skill/src/registry.rs`, `crates/agent/src/custom_agents.rs` — system prompt catalogs.
- `crates/tools/src/executor.rs` — `defs_cache`, `deferred_catalog`.
- `crates/cli/src/cmd/helpers.rs`, `crates/tui/src/run/persist.rs` — `refresh_session_memory`.

## Known limitations / not enforced

- Nothing asserts the prefix is stable: no hash of (tools, system) is kept
  between steps, so the only signal is the after-the-fact miss notice, and
  only on Anthropic.
- Rows 1, 7 and 8 of the audit are self-inflicted misses that the code does
  not avoid: a tool activation, a mid-turn user message, or simply the second
  step of an intent-annotated turn.
- The date line guarantees at least one full miss per day per session.
- The system prompt is rebuilt from disk before each non-trivial turn; any
  change to an instruction file, skill or `MEMORY.md` invalidates the whole
  history.
- Cache usage from keep-alives is folded into the session totals; a keep-alive
  that lands while the provider has already evicted the prefix pays a full
  write (not verified against provider billing).
- Keep-alive requests inherit `step_tools()` at send time, so an activation
  after the last turn makes the keep-alive write a *different* prefix than the
  one the next turn will send (not verified in practice).
- OpenAI-compatible providers may cache implicitly, but the agent cannot see
  it and `/cost` shows no cache columns for them.
- Subagent requests use the session default `use_prompt_cache: true`
  (`session.rs:947`) and a system prompt that embeds query-dependent recall
  (`crates/agent/src/subagent.rs:230-237`), so worker prefixes are
  per-task.

## Related

- [`../guide.md#slash-commands`](../guide.md#slash-commands) — `/fresh`, the "Prompt cache miss" line, `cache_warm_minutes`, `preconnect`.
- [`../guide.md#memory`](../guide.md#memory) — note: it says semantic facts are injected into the system prompt; the code puts recall on the user message (`service.rs:689-694`).
- [turn-loop.md](turn-loop.md) — request assembly (§5) and events.
- [compaction.md](compaction.md) — in-place rewrites and carriers.
- [retry.md](retry.md) — retries re-send the same request bytes.
- [permission-gate.md](permission-gate.md), [mcp-lifecycle.md](mcp-lifecycle.md) — tool list changes.
- [`../knowhow.md`](../knowhow.md).
