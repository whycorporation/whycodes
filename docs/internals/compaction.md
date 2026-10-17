# Context compaction

> Status: Implemented · Updated: 2026-10-17

Compaction is how a session keeps fitting into the model's context window.
There are two methods. *Full-replace* (the default) summarizes the whole
transcript (by the session model when possible, by a local stub otherwise)
and rebuilds the message list as `[last real user prompt, its tail,
summary carrier]`. *Provider-native* (opt-in, `compaction_method = "auto"`,
Anthropic only, §4a) asks the provider to summarize the messages before the
last real user prompt and replaces them with the signed block it returns.
Cheaper measures run before both on every step: oversized tool results are
capped, older ones are pruned, and under pressure "shaken".
Everything below is read from the code; unconfirmed details say "not verified".

## How it works

### 1. Triggers

| Trigger | Where | Note to the summarizer |
|---|---|---|
| Manual `/compact [note]` (alias `/summarize`) | TUI: `crates/tui/src/run/slash.rs:354-362` → `start_compact_task` in `crates/tui/src/run/mod.rs:5042-5085`; plain REPL: `crates/cli/src/cmd/run.rs:1012-1043` | the text after `/compact`, passed as `user_context` |
| Autocompact (over threshold) | `Agent::run_turn_steps`, `crates/agent/src/agent/turn.rs:222-268`, at the top of every LLM step | none |
| Context-overflow retry | same loop, three sites: stream open failed (`turn.rs:363-379`), stream item error (`turn.rs:472-494`), in-band `StreamEvent::Error` (`turn.rs:597-619`) | none |

Manual compaction never checks a token threshold (`compact.rs:66`). All three
triggers call the same `Agent::compact_session`
(`crates/agent/src/agent/compact.rs:70-97`).

### 2. The per-step cheap pass (runs before any LLM step)

```
truncate_large_tool_results   every tool body  <= 32_768 chars   (TOOL_RESULT_MAX_CHARS)
spill_old_tool_results        all but last 4 tool msgs > 2_048 chars: full body written to
                              .whycodes/scratch/artifacts/<tool_call_id>.txt once; the
                              message keeps 400 chars + "[N more chars; full output saved:
                              read <path>]"                        (SPILL_HEAD_CHARS, SPILL_MARK)
prune_old_tool_results        all but last 4 tool msgs <= 2_048  (TOOL_RESULT_PRUNE_CHARS)
shake_old_tool_results        all but last 4 tool msgs <= 512    (TOOL_RESULT_SHAKE_CHARS)
                              only when tokens > 3/4 * compaction_threshold
```

The spill runs before prune and shake, so a long old output is not cut in
place and lost: the model can `read` the artifact instead of re-running the
tool. The store lives in the agent (`crates/agent/src/agent/spill.rs`); the
session only receives a `store(id, body) -> Option<pointer>` callback and
keeps the body when the store declines (unwritable scratch dir). A spilled
body is under the shake cap and carries `SPILL_MARK`, so later passes leave
it alone.

`turn.rs:212-221`; constants and bodies in `crates/session/src/session.rs:15-25`,
`1048`, `1109-1158`, `1160-1208`. A cut body ends with `\n[N chars truncated]`
(`session.rs:193-195`); re-cutting keeps the original omitted count
(`split_truncation_notice`, `session.rs:202-225`). Token counts are the
chars/4 heuristic (`whycodes_core::tokens::estimate_tokens`,
`crates/core/src/tokens.rs:13-20`) over system prompt + messages, cached per
message (`Session::token_count_cached`, `session.rs:1035-1038`).

### 3. Autocompact decision and circuit breaker

```
before = token_count_cached()
if compaction_threshold > 0 && !compact_paused && before > compaction_threshold:
    outcome = compact_session(...)
    compact_failures = still_over ? compact_failures + 1 : 0        # next_compact_failures, turn.rs:960-966
    if compact_failures >= MAX_CONSECUTIVE_COMPACT_FAILURES (3):   # mod.rs:158
        compact_paused = true   -> Status "Auto-compact paused after 3 passes (~N tok still over threshold)"
```

`turn.rs:222-268`. The breaker is a local of `run_turn_steps`, so it resets
on the next user turn.

**Speculative summary.** When the step is under the threshold but at or
above `SPECULATIVE_LEAD_PERCENT` (85%) of it, compaction uses the model and
native compaction is not enabled for the step's model,
`maybe_spawn_speculative_summary` (`compact.rs`) starts a background task
that summarizes the *prefix* — the messages before the last real user turn,
`Session::prefix_transcript_for_summary` — with the same prompt and
transport as a normal pass. One job per (session, prefix) is kept in
`Agent::speculative_summary`; a newer prefix replaces an older job. When the
threshold trips, `take_speculative_summary` uses the job if its prefix is
still the session's (`compaction_prefix_end`), awaiting it when it has not
finished; otherwise the job is aborted and the normal pass runs. The
prepared summary goes through `compact_with_summary`: the same cheap passes
and `pre_compact` hooks, then `apply_full_replace`, with no model call at
the threshold. `CompactOutcome::still_over` is
`tokens_after > threshold` (`session.rs:64-66`). A successful pass emits
`TurnEvent::Status("Compacted context (a → b msgs, ~x → ~y tok)…")`
(`turn.rs:229-238`); only a status line, not a transcript rebuild (see §8).

The speculative job stays local-only. A native block is signed for exactly
the messages it was asked about, so preparing one ahead of the threshold
would need the provider's background recipe (record how many messages were
sent, then swap exactly that many) on top of the prefix tracking here. With
native compaction on, no job is started and the threshold pass asks the
provider directly.

### 4. `compact_session` (full-replace)

`crates/agent/src/agent/compact.rs:70-97`, in order:

1. `truncate_large_tool_results`, `prune_old_tool_results` (again).
2. Empty session → default `CompactOutcome`, nothing else runs.
3. `run_pre_compact_hooks` (`crates/agent/src/agent/lifecycle.rs:50-57`):
   every `[[hooks]]` with `event = "pre_compact"`, no extra env, cannot block,
   feedback ignored. Then `native_compact` (§4a); when it returns an
   outcome, the pass ends there.
4. `transcript = session.transcript_for_full_summary(0)` — the *whole*
   message list rendered as `Role: body` lines, capped at 48 000 chars
   (`FULL_TRANSCRIPT_MAX_CHARS`, `session.rs:38`, `1257-1265`, `513-545`).
   Tool-use blocks contribute only the tool name.
5. `local = session.local_full_replace_summary()` is always computed first
   (`session.rs:1280-1289`), as the fallback.
6. `want_llm = compaction_llm && api_key non-empty && transcript non-empty`.
   If so, `llm_compact_summary`; an empty or failed reply falls back to `local`.
7. `session.apply_full_replace(&summary)`.

### 4a. Provider-native compaction (`native_compact`)

`crates/agent/src/agent/compact.rs`, after the cheap passes and the
`pre_compact` hooks. It runs only when `compaction_method = "auto"`,
`compaction_llm` is on, the key is non-empty and
`detect_capabilities(provider, model).compaction` is true (first-party
`anthropic` provider, a model on the list of the on-demand compaction page,
`crates/llm/src/capabilities.rs`). Every `None` below falls through to
full-replace with a debug log:

1. Cut: `Session::native_compaction_prefix` — the same cut as full-replace
   (the last real user turn), but only when something precedes it and the
   message just before it is an assistant reply without a `tool_use` (the
   API rejects a summarized range that ends on an open call, and the first
   kept message must differ in role from the last summarized one). The
   prefix goes through the same tool-pairing repair as `build_request`.
2. Request: `build_request(step_tools(), max_tokens 8192, temperature)`
   with `messages` = the prefix, the agent's prompt-cache setting and
   `attach_thinking_request` — the same system prompt, tools and thinking
   settings as a turn, which the API requires for thinking in the kept turns
   to stay valid. `instructions` = the full-replace prompt without the
   transcript (`compact_instructions`), the `/compact` note capped at 8,000
   characters so the whole stays under the 16,384-character limit.
3. Call: `LlmTransport::compact` → `LlmProvider::compact` (redactor, 2
   retries, 120 s timeout, never the response cache). The Anthropic body is
   `compaction_body` (see `docs/providers.md`), header
   `anthropic-beta: compact-2026-09-04`.
4. Result: `stop_reason == "compaction"` and exactly one
   `ContentBlock::Compaction` (`native_compaction_block`). HTTP errors,
   `max_tokens` / `tool_use` / `refusal` / `end_turn` stops with empty content
   and extra blocks all fall through.
5. Swap: `Session::apply_native_compaction(cut, block)`:

```
before:  [m0 … m(k-1)] [U_last] [tail…]
after:   [Assistant: compaction block] [U_last] [tail…]      (unchanged, thinking included)
```

The block is stored as ordinary message JSON (`{"type":"compaction",
"content", "raw"}`) and sent back with `raw` verbatim on every later request
(with the beta header) while the model supports it; otherwise it degrades to
text (see `docs/providers.md`). The redactor leaves it alone (its signature
covers the text). Compacting again sends a prefix that starts with the old
block; the new block replaces it. A later full-replace pass reads the block's
text into its transcript.

### 5. The LLM summary call

`compact.rs:99-164`. One user message built by `build_compact_summary_prompt`
(`compact.rs:9-59`): no system prompt, no tools, `max_tokens 4096`,
`temperature 0.2`, `use_prompt_cache false`, through
`LlmTransport::complete` with a 60 s timeout and a 2-retry policy
(200 ms base, 3 s cap, 90 s elapsed cap). It uses the *session* provider and
model passed by the caller (the fallback model if one is active in that turn,
`turn.rs:372`).

The prompt asks for a `<summary>…</summary>` block with nine numbered sections
(request and intent, key concepts, files and code with full snippets of
changed code, errors and fixes, problem solving, all user messages, pending
tasks, current work, optional next step), forbids tool calls and a separate
analysis block, and tells the model to treat an earlier carrier
(`This session is being continued` / `[Compacted`) as authoritative. The
`/compact` note is inserted as **User-provided context for this compaction**.

Only `ContentBlock::Text` parts of the reply are kept (`compact.rs:146-156`).
`format_compact_summary` (`session.rs:277-314`) strips a leading
`<analysis>` block, unwraps `<summary>`, and squeezes blank lines;
`format_compact_summary_content` (`session.rs:316-326`) prepends
`COMPACT_CONTINUATION_PREAMBLE` (`session.rs:41-42`).

### 6. The local fallback summary

`summarize_trimmed` (`session.rs:349-…`) counts users/assistants/tools, keeps
up to 3 first-line user goals (120 chars, skipping `[…` stubs), up to 8 file
paths found in assistant/tool content, and up to 8 tool names. It is used when
`session.compaction_llm = "off"`, when there is no API key, when the model
call fails, or when the model returns no text. It summarizes the messages
*before* the last real user prompt (`local_full_replace_summary`).

### 7. What is kept and dropped (`apply_full_replace`)

`session.rs:1295-1361`:

```
before:  [m0 … m(k-1)] [U_last] [tail: assistant/tool/user after U_last]
after:   [U_last (text only)] [tail…] [User: PREAMBLE + summary]
```

- `U_last` = last `Role::User` message whose text is not a compaction carrier
  (`last_real_user_index`, `session.rs:1267-1278`). It is re-created from
  `content.as_text()`: a blocks message (text + image) loses non-text blocks.
- The tail after `U_last` is kept verbatim (including its tool results, after
  the caps of §2).
- Everything before `U_last` is dropped; its transcript (≤ 12 000 chars) is
  returned in `CompactOutcome::dropped_transcript` for diagnostics.
- The summary is appended as a **user** message at the end. With no tail this
  yields two adjacent user messages; not verified how every provider treats
  that.
- The system prompt is untouched (`session.system_prompt` lives outside
  `messages`); the token cache is rebuilt.

`prepend_compact_summary` and `Session::compact` (prefix-drop keeping
`MIN_KEEP_MESSAGES = 4`) still exist (`session.rs:1210-1248`, `1380-…`) but no
production call site uses them (only `session.rs` tests).

### 8. Transcript views vs stored session

- **Stored session**: `SessionRuntime::persist` → `Session::save_to_db` does
  a full replace of the `messages` rows (`crates/session/src/session.rs:1461-1483`,
  `crates/storage/src/db.rs:287-294`). After any compaction the database holds
  only the compacted list.
- **TUI after `/compact`**: `apply_compact_view` reloads the chat from the
  session (`load_messages_from_session`), so scrollback becomes the compacted
  view; carriers are painted as a system card via
  `compact_summary_display_text` (`crates/tui/src/run/mod.rs:3786-3800`,
  `crates/tui/src/app.rs:4068-4083`), then `rt.persist("compact")`.
- **TUI after autocompact / overflow compaction**: only a `Status` event
  reaches the UI (`turn.rs:229-238`, `365-370`), handled as a status line
  (`crates/tui/src/run/mod.rs:5290-5300`). Scrollback keeps the pre-compaction
  rows until the session is reloaded; the stored session already differs.
- **Plain REPL**: prints `compact_ok_line` and the summary card
  (`crates/cli/src/cmd/run.rs:1028-1042`); earlier terminal output stays.

### 9. Subagents and swarm

`SubagentRunner::run_turn_inner` (`crates/agent/src/subagent.rs:300-…`) has no
threshold check, no overflow handling and no compaction: it builds the request
from the worker session and streams with the default transport. A worker that
overflows fails its task with the provider error. Swarm workers go through the
same runner (`crates/agent/src/agent/spawn.rs:57-90`). The parent folds worker
usage into its own session (`turn.rs:871-884`) but worker transcripts are never
part of the parent's context, so they do not drive the parent's compaction.

### 10. Design notes — methods that do not exist today

Stated as absences, not plans:

- Provider-native compaction exists for Anthropic's on-demand compaction
  only (§4a). Anthropic's threshold compaction (`context_management`) and
  context editing are not used. OpenAI's compaction is a Responses-API
  feature (`/v1/responses/compact`, encrypted items), which whycodes does not
  speak to api.openai.com; Gemini has no such primitive.
- No incremental/rolling summary: each pass re-summarizes the whole (capped)
  transcript, relying on the prompt to carry an earlier carrier forward.
- No token counting against the provider's tokenizer; thresholds use chars/4.

## Configuration and overrides

| knob | where | effect |
|---|---|---|
| `compaction_threshold` (default 150 000) | `[session]` config, `crates/config/src/types.rs:1331-1332`, `1544-1546` | autocompact when heuristic tokens exceed it; `0` disables autocompact and the shake (`turn.rs:214`, `222`) |
| `compaction_llm` (`"auto"` default; `off`/`false`/`0`/`none`/`local`) | `[session]`, `types.rs:1394-1398`; parsed in `crates/agent/src/agent/mod.rs:614-621` | `off` keeps the local stub only (and disables native compaction) |
| `compaction_method` (`"local"` default; `"auto"`), env `WHYCODES_COMPACTION_METHOD` | `[session]`, `crates/config/src/types.rs`; parsed in `Agent::apply_config` | `auto` tries provider-native compaction first (§4a) |
| `/compact [note]`, `/summarize` | slash command (TUI + REPL) | full-replace now, note forwarded to the prompt |
| `[[hooks]] event = "pre_compact"` | config hooks, `lifecycle.rs:50-57` | runs before the transcript is captured; cannot block |
| `max_context_tokens` (default 200 000) | `[session]`, `types.rs:1329-1330` | not read by compaction (grep); used elsewhere, not verified here |

There is no knob for the tool-result caps, the "last 4 tool messages" window,
the 48 000-char transcript cap, or the 3-pass breaker.

## Implementation files

- `crates/agent/src/agent/compact.rs` — `compact_session`, `native_compact`, `llm_compact_summary`, the summarizer prompt.
- `crates/llm/src/providers/anthropic.rs` — `compaction_body`, `compact`, block replay and the compaction beta.
- `crates/llm/src/capabilities.rs` — `ModelCapabilities::compaction`, `ANTHROPIC_COMPACTION_MODELS`.
- `crates/core/src/types.rs` — `ContentBlock::Compaction`.
- `crates/agent/src/agent/turn.rs` — per-step caps/shake, autocompact + breaker (`212-268`), overflow compact-and-retry (`363-379`, `472-494`, `597-619`).
- `crates/agent/src/agent/mod.rs` — `MAX_CONSECUTIVE_COMPACT_FAILURES`, `compaction_threshold` / `compaction_llm` from config.
- `crates/agent/src/agent/lifecycle.rs` — `run_pre_compact_hooks`.
- `crates/session/src/session.rs` — caps, prune, shake, `transcript_for_full_summary`, `local_full_replace_summary`, `apply_full_replace`, `CompactOutcome`, carrier formatting, `save_to_db`.
- `crates/llm/src/error_class.rs` — `looks_context_overflow` (`295-305`) decides what counts as overflow.
- `crates/tui/src/run/mod.rs`, `crates/tui/src/run/slash.rs`, `crates/cli/src/cmd/run.rs` — manual trigger and UI refresh.
- `crates/agent/src/agent/turn_tests.rs` — `auto_compact_pauses_after_failures`, `mid_stream_context_overflow_compacts_and_retries`, `overflow_fail_open_retries_then_succeeds`, `shake_and_compact_reset_with_tool_dumps`.

## Known limitations / not enforced

- Native compaction is off by default because these were not run against
  the live API (no test calls the network); run them before turning it on
  by default:
  1. A history whose first message is an `assistant` message holding the
     block is accepted, and `compaction_block_misplaced` fires when one
     summarized message is left in front of it.
  2. The block survives a storage round trip: no
     `compaction_signature_invalid`, `compaction_content_mismatch` or
     "Extra inputs" 400 after a session reload.
  3. The `instructions` (full whycodes prompt) with the full tool list yield
     `stop_reason: "compaction"` with text, not `tool_use`.
  4. `anthropic-beta: oauth-2025-04-20,compact-2026-09-04` is accepted on a
     subscription OAuth token; if the beta is API-key only, the capability
     must depend on the credential kind.
  5. `usage.iterations` is present and is what is reported; top-level
     tokens are zero.
  6. After the swap, `cache_read_input_tokens` shows the system prompt still
     cached.
  7. Kept thinking stays valid on a preserved-thinking model: whycodes
     appends ephemeral text (turn context, intent posture) to the latest user
     message per step, so the kept turns may not be byte-identical to what
     produced their thinking; a 400 here would surface on the next step.
  8. A model switch inside the list accepts the block; a switch to a model
     outside it sends the summary as user text without a 400.
  9. Compacting a history that already starts with a block returns one new
     block.
- The usage of a compaction call (native or local) is not added to the
  session cost.
- Gemini and Ollama render a message from its first text, so after a switch
  away from Anthropic the summary is sent as a leading assistant message;
  not verified how Gemini treats a model-first history.
- A binary older than this change cannot read a stored session holding a
  compaction block (unknown content block type).
- Native compaction sends the turn's thinking settings; with thinking on,
  `max_tokens 8192` is raised to budget + 4096 by `apply_anthropic`.
- Images and documents in the summarized range are gone after a native
  swap, as with full-replace.

- The summary call goes through `LlmTransport::complete`, which consults the
  process-local `ResponseCache` (exact or semantic ≥ 0.88, 600 s TTL,
  `crates/llm/src/transport.rs:72-82`, `crates/llm/src/response_cache.rs:18-22`).
  Two compactions of a near-identical transcript within ten minutes can
  replay the earlier summary text.
- Overflow is retried at most once per user turn (`overflow_retries < 1`); a
  second overflow in the same turn surfaces the provider error.
- A tail that is itself over the threshold cannot be reduced: full-replace
  keeps it verbatim, which is what the 3-pass breaker is for.
- The breaker emits a status once and then silently sends oversized requests
  for the rest of the turn.
- The local stub carries goals, paths and tool names only; pending tasks and
  decisions from the dropped prefix are lost.
- `U_last` is re-created as text: images attached to the last prompt are
  dropped by compaction.
- Hook feedback appended to a prompt (`<hook-context>`) and steering messages
  before `U_last` are summarized, not preserved.
- Caps and prune rewrite old tool results in place on every step, which also
  changes the cached prompt prefix the first time they fire (see
  [prompt-cache.md](prompt-cache.md)).
- Subagents never compact.
- The TUI scrollback is not refreshed after an automatic compaction.

## Related

- [`../guide.md#slash-commands`](../guide.md#slash-commands) — `/compact`, `/fresh`.
- [`../guide.md#configuration`](../guide.md#configuration) — "Context-window errors are classified separately from 429s".
- [`../guide.md#hooks`](../guide.md#hooks) — `pre_compact`.
- [turn-loop.md](turn-loop.md) — step order around the compaction call.
- [retry.md](retry.md) — overflow classification and compact-and-retry.
- [prompt-cache.md](prompt-cache.md) — why in-place rewrites cost cache reads.
- [permission-gate.md](permission-gate.md), [mcp-lifecycle.md](mcp-lifecycle.md).
- [`../knowhow.md`](../knowhow.md) — `max_tokens` vs `context_window`.
