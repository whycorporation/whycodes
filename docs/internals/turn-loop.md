# Turn loop

> Status: Implemented · Updated: 2026-10-16

This page describes what happens between a user pressing Enter and the agent
returning its final text: hook points, context assembly, streaming, tool
execution, loop termination, and what the UI is told along the way. It is for
contributors changing `crates/agent`; the user-facing contract lives in
[`docs/guide.md`](../guide.md). Everything below is read from the code; where
a detail was not confirmed it says "not verified".

## How it works

Vocabulary: a **turn** is one user prompt and everything until the model
answers without tool calls. A **step** is one LLM request inside a turn. The
`Agent` facade (`crates/agent/src/agent/mod.rs`) sits in the orchestration
layer; it drives `whycodes-llm`, `whycodes-session`, and `whycodes-tools`
(see [architecture](../architecture.md)).

```
run_turn_with_events
  ├─ run_prompt_hooks           session_start (once) → user_prompt_submit (may block)
  ├─ run_turn_steps
  │    ├─ per-turn setup        magic keywords, routing, intent, tools-free check
  │    └─ loop (step)
  │         ├─ inject background completions + steering messages
  │         ├─ tool-result hygiene, auto-compact
  │         ├─ build request (+ turn context, intent posture, magic notice)
  │         ├─ open stream (cancel race, overflow/429/fallback handling)
  │         ├─ stream body (text / thinking / tool-call assembly / speculative reads)
  │         ├─ stream-rule or harmony retry → restart step
  │         ├─ assistant message appended; no tool calls → break
  │         ├─ doom-loop guard → refuse batch
  │         ├─ execute_tool_calls (parallel reads / serialized mutators)
  │         └─ results appended (+ nested AGENTS.md, checkpoint/rewind, failures)
  └─ run_stop_hooks             stop (never blocks)
```

### 1. Entry points

`Agent::run_turn` and `Agent::run_turn_with_events`
(`crates/agent/src/agent/turn.rs`) take a `TurnOpts`
(`crates/agent/src/events.rs`): provider, model, API key, `max_turns`
(`None` = unlimited, which is what the TUI passes), an optional `EventSink`
(unbounded mpsc of `TurnEvent`) and an optional `CancelFlag`
(`Arc<AtomicBool>`, set by Esc / `[stop]`). The prompt is the text of the last
`Role::User` message already in the `Session`; the caller appends that message
before calling.

### 2. Prompt hooks

`run_prompt_hooks` (`crates/agent/src/agent/lifecycle.rs`) runs only when
`[[hooks]]` is non-empty:

- `session_start` once per session id per process (`Agent::session_starts`
  set), then `user_prompt_submit` with `WHYCODES_PROMPT` in the environment.
- A `user_prompt_submit` hook with `block_on_failure = true` that exits
  non-zero (or times out) returns `Error::Agent("user_prompt_submit hook
  blocked …")` before any LLM call.
- `feedback = true` stdout of both events is appended to the latest user
  message inside `<hook-context>` tags (`add_turn_context`), so it is part of
  the transcript from then on.

### 3. Per-turn setup (`run_turn_steps`)

Computed once before the step loop:

- `magic_keywords::scan` on the last user text (`crates/agent/src/magic_keywords.rs`):
  standalone `ultrathink` / `orchestrate` outside code and markup set a
  `MagicHit`.
- `skip_prompt_cache_once` is swapped to `false`; a `/fresh` sets it so the
  next step bypasses provider prompt cache and the local response cache.
- `routing::resolve_agent_model`: the `plan` agent uses `session.model_plan`
  when set; everything else keeps the session route. With `session.prewalk`
  every agent takes that `plan` route (the strong model).
- `prewalk_resume` (`crates/agent/src/agent/prewalk.rs`): when an earlier
  turn handed off from this same strong route, the turn's `fallback` slot
  starts on that cheap target, with `Status("Prewalk: continuing on …")`.
  `/prewalk restart` (`Agent::prewalk_restart`) clears it.
- `tools_free_chat`: the user text is a trivial seed
  (`title::is_trivial_title_seed`), the session has at most one user message,
  and the transcript has no tool work. Then the request carries no tool
  schemas at all.
- `intent::classify_user_intent` produces an `IntentAssessment`
  (`question` / `change` / `plan` / `ambiguous` / `trivial` plus confidence).
  `TurnEvent::Intent` is emitted immediately with badge and optional notice.
  The assessment is later handed to the tool gates (see
  [permission-gate.md](permission-gate.md)).
- Provider lookup in the registry; counters for doom-loop signatures
  (`VecDeque`, max 16), compaction circuit breaker, overflow retries (max 1),
  harmony retries (max 2), and a per-turn model `fallback` slot.

### 4. Start of every step

In order, at the top of the `loop`:

1. `inject_auto_background_completions` (`agent/mod.rs`): finished
   auto-detached shell jobs not yet delivered become a user message
   `Background job \`id\` finished (status). Output: <last 4000 chars>`.
2. `inject_steering`: `SteerInbox::drain(session.id)`
   (`crates/agent/src/steer.rs`) appends each message the user sent while the
   turn was running as a user message and emits
   `Status("message delivered to the running turn")`. Whatever is still queued
   when the turn ends is taken back by the UI and sent as the next turn.
3. `step_tools`: `ToolExecutor::get_definitions_profile_extra(permission,
   tool_profile, activated)` — the configured profile (`core` by default)
   plus tools activated through `tool_search` this session; the `swarm`
   definition is stripped when `[swarm] enabled = false`.
4. `tool_context` builds the `ToolContext` (`crates/core/src/tool.rs`):
   working dir (or the `worktree enter` override), sandbox settings (network
   forced off when the agent's `permission.allow_network` is false), HTTP
   network policy, file-claim registry for parallel TUI sessions, file index,
   panel/todo sinks.
5. Cancel check → `TurnEvent::Cancelled` + `Error::Agent("Cancelled")`.
6. `turn_count += 1`; with `max_turns = Some(n)`, step `n+1` fails with
   `Exceeded maximum turns`.
7. Tool-result hygiene on the session: `truncate_large_tool_results`,
   `prune_old_tool_results`, and `shake_old_tool_results` once the cached
   token count exceeds 3/4 of `compaction_threshold`.
8. Auto-compact: when `compaction_threshold > 0` and the session is over it,
   `compact_session` (`crates/agent/src/agent/compact.rs`; runs `pre_compact`
   hooks, LLM summary or local stub) runs before the request. Three
   consecutive passes that stay over the threshold trip
   `MAX_CONSECUTIVE_COMPACT_FAILURES` and pause auto-compact for the rest of
   the turn. Details: [compaction.md](compaction.md).

### 5. Request assembly

`session.build_request(tools, None, temperature, Some(true))` produces the
`LlmRequest`; then, on the request copy only (the session transcript is not
modified, which keeps the cached prefix stable):

- `append_turn_context` adds `session.turn_context` (memory recall computed
  by the host per prompt) to the latest user message.
- `use_prompt_cache` unless `/fresh`.
- `thinking_acc::attach_thinking_request` sets provider-specific thinking /
  reasoning-effort fields; `apply_ultrathink` when the keyword matched.
- Step 1 only: `intent::posture_suffix` when `should_inject(intent_guidance,
  assessment)` says so, and the magic-keyword notice (`<whycodes_keyword>`).

The system prompt itself is built by the host (`Agent::system_prompt_for_route`,
`with_agents_md`, memory injection) and stored on the `Session` before the turn;
the turn loop does not rebuild it. Prompt-cache behaviour:
[prompt-cache.md](prompt-cache.md).

### 6. Opening the stream

`whycodes_llm::default_transport().stream_turn(target, &request, opts)` is
raced against `wait_until_cancelled` in a `biased` `select!`, so a hung
gateway cannot ignore Esc. Options: the process-local response cache (only for
tool-free requests and not after `/fresh`) and an optional first-token race
partner (`session.model_race`, `race_after_ms`). Open failures are classified
(`whycodes_llm::classify`):

| Error kind | Action |
|---|---|
| `ContextOverflow` (first time this turn) | `compact_session`, `Status("Context overflow — compacting and retrying this step…")`, restart step |
| `RateLimited` | `failover_api_key` picks the next named credential for the same provider; sticky credential updated; restart step |
| any `retryable()` kind | `next_fallback` walks `session.fallback_models` from the slot's next index (0 after a prewalk hand-off); the chosen target sticks for the rest of the turn; restart step |
| otherwise | the turn returns `Err(e)` |

Transport-level retry/backoff is inside `whycodes-llm`: [retry.md](retry.md).

### 7. Stream body

Every event is awaited inside another `biased` `select!` against the cancel
flag (`wait_until_cancelled` polls every 40 ms). Handling per
`StreamEvent` (`crates/core/src/types.rs`):

| Event | Effect |
|---|---|
| `TextDelta` | flush pending thinking, record TTFT, emit `TurnEvent::TextDelta`, append to `accumulated_text`, check stream rules |
| `ToolUse` / `ToolUseDelta` | `ToolCallAssembler` (`crates/agent/src/tool_stream.rs`) merges Anthropic-style and OpenAI-style fragments; after each delta `speculative_read::maybe_start` may start an early `read` |
| `Thinking` / `ThinkingDelta` / `ThinkingSignature` / `RedactedThinking` | `ThinkingAccumulator` (`crates/agent/src/thinking_acc.rs`) collects blocks; `TurnEvent::ThinkingDelta` is emitted for text |
| `Usage` / `CacheUsage` | folded as a snapshot max (providers repeat or split usage) |
| `MessageStart` / `MessageDelta` | ignored |
| `MessageStop` | end of step |
| `Error { message }` | context overflow → compact and retry once; otherwise `Err` |

Speculative reads (`crates/agent/src/speculative_read.rs`): as soon as a
`read` call's `path` string is closed in the partial JSON, and the file
exists, a `ReadTool` job is spawned with the same `ToolContext`. The result is
consumed later only when the finalized call has the same id, canonical path,
offset and limit, and names a window (`offset`/`limit`) exactly when the
early read did, since a windowless read of a long source file returns an
outline; otherwise the job is aborted. A call carrying `outline` is never
speculated. They are never started when a
`pre_tool` / `post_tool` hook matches `read` (`Agent::read_has_hooks`).

Stream rules (`session.stream_rules`, compiled in `agent/mod.rs`): after every
text delta `first_stream_rule_hit` tests the accumulated text against each
regex. On a hit `apply_stream_rule_hit` aborts speculative reads, emits
`Status("Stream rule `name` interrupted the draft")`, appends a user message
`<whycodes_rule name=…>hint…</whycodes_rule>`, discards the draft (it is never
added to the session) and restarts the step.

Cancel during streaming: speculative reads aborted, the thinking blocks and
partial text are appended as an assistant message, `TurnEvent::Cancelled`,
`Err("Cancelled")`.

### 8. After the stream

1. `assembler.finish()` parses argument buffers, assigns fallback ids, and
   renames `file_path` / `filepath` / `file` to `path` for built-in file
   tools.
2. Harmony-dialect providers: `harmony_leak` scans text, thinking and tool
   arguments for protocol leakage; the draft is discarded and retried up to two
   times, then the turn errors.
3. Tool-free replies are stored in `ResponseCache::global()`.
4. `TurnEvent::ToolStart` is emitted per call with the final parsed arguments
   (not the empty first chunk).
5. Usage and USD cost (`step_cost`, config `[pricing]` wins over the catalog)
   are added to the session once per step; `TurnEvent::Usage` and a cache-read
   note (`note_cache_read`) follow.
6. The assistant message is appended: thinking blocks, then text, then one
   `ContentBlock::ToolUse` per call. An empty message is never appended.
7. No tool calls → a `turn.step` log record with `done: true` and `break`.

### 9. Doom-loop guard

`would_doom_loop` (`agent/mod.rs`): the signature is `name|json(args)`. A
batch in which every call has the same signature, and that signature already
closes out the recent history, counts; when the run length reaches
`DOOM_LOOP_THRESHOLD` (3) the whole batch is refused without executing:
`Status("Doom loop: identical tool call repeated — refusing")`, one error
`ToolEnd` and one error `ToolResult` per call telling the model to change
approach. Mixed batches and alternating patterns are not detected.

### 10. Tool execution scheduling

`Agent::execute_tool_calls` (`crates/agent/src/agent/gate.rs`) returns
results in call order:

- One call: `Status("Running tool …")`, speculative hit or
  `execute_with_permission`, raced against cancel.
- All calls parallel-safe (`is_parallel_safe_tool`: not in `SERIAL_TOOLS`,
  `crates/agent/src/tool_policy.rs` — shell, write/edit/apply_patch,
  git_commit, todo tools, task, swarm, bg, schedule, tool_search, worktree,
  plan, browser, question, code_mode, skill, external_directory, memory):
  speculative results are consumed first, the rest run through
  `futures::join_all`, the whole batch raced against cancel.
- Mixed batch: strictly sequential, cancel checked before each call.

Every gate a call passes inside `execute_with_permission` is listed in
[permission-gate.md](permission-gate.md). `dispatch_tool` then routes `task`,
`swarm`, `bg`, `read` of an `mcp://` URL (`execute_mcp_read`, the MCP clients
live on the agent), `schedule`, `tool_search`, `worktree`, background and
auto-background shell in-process (`crates/agent/src/agent/dispatch.rs`) and
everything else to `ToolExecutor::execute`. The other internal `read` URLs
(`skill://`, `agent://`, `proc://`, `rule://`, `pr://`, `issue://`) resolve
inside the tool (`crates/tools/src/file/internal.rs`).

Prewalk hand-off: after a batch has run, and only while the `fallback` slot
is empty, `prewalk_target` checks the batch for a mutating call
(`prewalk::is_mutating_call`: `edit`, `write`, `apply_patch`, `ast_edit`, or
`bash` that `whycodes_command_risk::assess` does not rate `Safe`). The call
itself already ran on the strong model. The cheap target
(`routing::resolve_worker_model` with `session.model_smol`) goes into the same
`fallback` slot a failed open would use, so every later step of the turn uses
it, and is remembered for later turns. It emits
`Status("Prewalk: handing off to <provider/model> after the plan")`. No
cheaper sibling, an unregistered provider, or no key (`Agent::target_key`, the
same lookup as `next_fallback`) keeps the strong model with a debug log.

If the batch returns `Err` (cancel), `close_unanswered_calls` appends an error
result `Tool not run: the turn was stopped before this call finished.` for
every call in the batch so the next request is not rejected by strict
providers, then the error propagates.

### 11. Appending results

In order: `attach_nested_instructions` (the first successful `read` into a
nested directory gets that directory's `AGENTS.md`-style files appended,
`crates/agent/src/context_files.rs`); `settle_checkpoint_rewind` validates
`checkpoint` / `rewind` calls; failed results are collected;
`session.add_tool_results`; `mark_checkpoint` / `apply_rewind`; pending
subagent usage and cost are folded into the turn and session; finally, if any
tool failed, a user message `The tool failed with error: … Please correct your
approach.` is appended. Then the loop continues with the next step.

### 12. Turn end

A `turn.done` structured log (`whycodes_core::logging::emit_sid`), a desktop /
chat notification (`notify::spawn_turn_done`), and
`memory_retain::spawn_post_turn_retain` (asynchronous, never awaited) run; the
final text is each step's text joined by a blank line (`append_step_text`).
`run_turn_with_events` then runs `stop` hooks with `WHYCODES_RESPONSE`.

### 13. Persistence

The agent never touches storage. Each host persists after
`run_turn_with_events` returns, best-effort (failures are logged):

- TUI: `SessionRuntime::persist(reason)` (`crates/tui/src/session_runtime.rs`)
  with reasons such as `ok`, `cancelled`, `error`, `compact`, `background`,
  `switch`, `close`, `exit`.
- CLI `run` / `generate`: `session.save_to_db` after the turn
  (`crates/cli/src/cmd/run.rs`).
- Daemon: `routes::save_session` after a turn and on mutations
  (`crates/server/src/routes.rs`, `v1.rs`).

Nothing is written during a turn; a process crash mid-turn loses the turn's
messages.

### 14. Events emitted to the UI

`TurnEvent` (`crates/agent/src/events.rs`): `TextDelta`, `ThinkingDelta`,
`ToolStart`, `ToolEnd`, `Usage` (once per step), `Status`, `Intent`,
`Cancelled`, `Background` (job started / done / failed / killed),
`EnqueuePrompt` (`schedule` goal), `Panel`, `Todos`, `Subagent` (running /
completed / failed), `SwarmStatus`, `SwarmMessage`, `FileConflict`,
`FileStale`, `PermissionAsk` and `QuestionAsk` (daemon → SDK client). `emit`
drops events silently when the receiver is gone.

### 15. What `task` / `swarm` subagents reuse

`execute_task_tool` and `execute_swarm_tool` (`dispatch.rs`) build a
`SubagentRunner` (`crates/agent/src/subagent.rs`) and call
`run_turn_inner`, a separate loop that reuses:

- the same `ToolExecutor`, `ProviderRegistry`, sandbox and network policy,
  memory settings, file index, hooks, question prompter and approval mode;
- `ToolCallAssembler`, `Session::build_request`, usage folding;
- steering (`with_steer`, keyed by `task-<call id>` or `worker-N`), the swarm
  mailbox (`SwarmHub`), and file claims in same-checkout swarm mode;
- a parallel/serial split equivalent to `SERIAL_TOOLS`, pre/post tool hooks,
  `subagent_stop` hooks;
- `max_turns` (the `task` tool defaults to 15), worker model
  `session.model_smol`, per-type permission profiles (`explore` / `scout`
  read-only; `general` = parent minus todo / task / swarm; custom agents from
  `custom_agents.rs`). A custom agent with `prewalk: true`
  (`CustomAgent::prewalk_route`) starts on the strong route and
  `run_turn_inner` switches to the worker model after the first step whose
  calls include a mutating one (`with_prewalk`).

It does **not** reuse: streaming `TurnEvent`s (thinking is only logged),
speculative reads, stream rules, the doom-loop guard, auto-compact and the
overflow / rate-limit / fallback handling, the cancel flag, or any of the
`execute_with_permission` gates. The worker's output is returned as the tool
result, persisted under `.whycodes/agents/<id>.md`, and its usage is folded
into the parent at the end of the batch.

With an output schema (`task` `output_schema`, or a custom agent's `output`),
`SubagentRunner::run` appends the `# Structured result` instruction to the
worker's system prompt and, after `run_turn_inner` returns, checks the last
` ```json ` block (`crates/agent/src/structured_output.rs`). A failure adds
one user message with the errors and runs `run_turn_inner` once more with
`max_turns = 1`; its usage is added to the worker's. The outcome is
`SubagentResult::structured` (`value` or `errors`); a valid value is written to
`.whycodes/agents/<id>.json` and appended compactly to the tool result. The
run itself stays `success` either way. `Agent::spawn_subagent` /
`spawn_parallel` (`crates/agent/src/agent/spawn.rs`) expose the same runner to
hosts.

`/btw` side turns (`crates/agent/src/side_turn.rs`) are a third, non-streaming
loop: a scratch session with read-only tools, at most four steps, never
written back to the parent session.

## Configuration and overrides

| Knob | Where | Effect |
|---|---|---|
| `max_turns` | CLI `run` / `generate` `-t` / `--max-turns`; `TurnOpts.max_turns` | Step cap; `None` in the TUI |
| `[session] compaction_threshold` | config | Auto-compact above this estimate; `0` disables; 3/4 of it triggers `shake_old_tool_results` |
| `[session] compaction_llm` | config | `off`/`false`/`0`/`none`/`local` → local stub instead of LLM summary |
| `[session] tool_profile` | config | Tool schema set per step (`core` default); `tool_search` adds extras |
| `[session] prompt_cache` | config | `none`/`off`/`false`/`0` disables provider cache markers |
| `[session] response_cache` | config | Process-local cache for tool-free replies |
| `[session] model_race`, `race_after_ms` | config | First-token race partner and delay |
| `[session] fallback_models` | config | `provider/model` list tried when a stream fails to open |
| `[session] model_plan` | config | Route used while the `plan` agent is active |
| `[session] model_smol` | config | Model for `task` / `swarm` workers |
| `[session] prewalk`, `--prewalk`, `WHYCODES_PREWALK` | config / CLI / env | Plan on the strong model, hand the turn to the cheap one after the first edit |
| `[session] reasoning_effort` | config | Passed to `attach_thinking_request` |
| `[session] intent_guidance` | config | `auto` (default) / `off` / `always`: posture injection and tool authorization |
| `[session.magic_keywords] enabled`, `ultrathink`, `orchestrate` | config | Keyword notices; `ultrathink` also raises thinking effort |
| `[[session.stream_rules]] name`, `pattern`, `hint` | config | Regex abort/retry of a streaming draft |
| `[tools.bash] auto_background`, `auto_background_after_secs` | config | Detach a still-running foreground shell (default on, 20 s) |
| `[automation] max_background_jobs` | config | Cap on concurrent background jobs |
| `[swarm] enabled`, `max_agents`, `worktrees` | config | `swarm` tool availability, concurrency, worktree isolation |
| `[[hooks]]` with `event = session_start` / `user_prompt_submit` / `stop` / `pre_compact` / `subagent_stop` | config | Lifecycle hooks around the turn |
| `[pricing."provider/model"]` | config | USD per million tokens used by `step_cost` |
| `/fresh` | TUI slash command (`Agent::skip_prompt_cache_next`) | Skip prompt and response cache for the next step |
| Esc / `[stop]` | TUI | Sets the `CancelFlag` |
| steering | TUI prompt while a turn runs; `POST /api/session/{id}/steer` | Pushes into `SteerInbox` |

## Implementation files

- `crates/agent/src/agent/turn.rs` — `run_turn`, `run_turn_with_events`, `run_turn_steps`: the step loop, stream handling, retries, result appending.
- `crates/agent/src/agent/mod.rs` — `Agent` struct and config application; steering / background injection; `would_doom_loop`, `first_stream_rule_hit`, `settle_checkpoint_rewind`, `tool_context`, fallback and race resolution.
- `crates/agent/src/agent/gate.rs` — `execute_tool_calls` scheduling and `execute_with_permission` gates.
- `crates/agent/src/agent/dispatch.rs` — in-process tools: background shell, `bg`, `read mcp://`, `schedule`, `tool_search`, `worktree`, `swarm`, `task`.
- `crates/agent/src/agent/lifecycle.rs` — `session_start`, `user_prompt_submit`, `stop`, `pre_compact` hooks.
- `crates/agent/src/agent/compact.rs` — `compact_session` (auto and `/compact`).
- `crates/agent/src/agent/spawn.rs` — public `spawn_subagent` / `spawn_parallel`.
- `crates/agent/src/subagent.rs` — `SubagentRunner::run_turn_inner`, the worker loop.
- `crates/agent/src/events.rs` — `TurnEvent`, `TurnOpts`, cancel flag helpers.
- `crates/agent/src/tool_stream.rs` — `ToolCallAssembler`.
- `crates/agent/src/speculative_read.rs` — early `read` jobs while arguments stream.
- `crates/agent/src/thinking_acc.rs` — thinking block accumulation and harmony-leak scan.
- `crates/agent/src/steer.rs` — `SteerInbox`.
- `crates/agent/src/background.rs` — `BackgroundRegistry`, auto-detach, completion delivery.
- `crates/agent/src/intent.rs` — intent classification, posture suffix, `authorize_tool`.
- `crates/agent/src/magic_keywords.rs` — `ultrathink` / `orchestrate` detection.
- `crates/agent/src/routing.rs` — plan / worker / fast model routing.
- `crates/agent/src/side_turn.rs` — `/btw` read-only side loop.
- `crates/agent/src/context_files.rs` — nested instruction files attached to `read` results.
- `crates/agent/src/memory_retain.rs` — post-turn memory retain (async).
- `crates/session/src/session.rs` — `Session`: `build_request`, tool-result hygiene, checkpoints (not read in detail for this page).

## Known limitations / not enforced

- Stream-rule retries are unbounded in interactive sessions: every hit restarts
  the step and only a headless `max_turns` caps it. The doom-loop guard only
  looks at tool calls, so a rule that keeps matching text-only drafts loops.
- Context-overflow recovery runs once per turn (`overflow_retries < 1`); a
  second overflow, or an overflow when auto-compact is paused, ends the turn
  with the provider error.
- The doom-loop guard only recognises a pure repeat (same name and arguments,
  three in a row). Alternating calls or batches with one differing call are
  executed.
- Cancel while a parallel batch is in flight drops results of tools that
  already finished; every call in the batch gets the `Tool not run` result.
  Dropping the shell future does not kill the child process: the in-spawn
  timeout (default 120 s) does (`crates/tools/src/shell.rs`).
- A speculative read consumes its result without passing
  `execute_with_permission` (`take_speculative_read` runs first), so
  tool-name and path permission rules for `read` are not applied on that path;
  only hooks are respected (by disabling speculation). The file is read at
  stream time, so a write that lands between speculation and the final call is
  not visible.
- `tools_free_chat` sends no tool schemas on a trivial first message; the
  model cannot call a tool on that step even if the heuristic was wrong.
- Background completions are injected as user messages with the last 4000
  characters only; the full log lives in the job registry (`bg read`).
- Subagent usage is folded only at the end of a batch
  (`take_pending_usage`); when the parent batch errors, the pending usage is
  attributed to a later turn. Not verified whether anything clears it.
- The subagent and side-turn loops are separate implementations and already
  differ from the primary loop (no speculative reads, stream rules, doom-loop
  guard, compaction, fallback, cancel). Changes to `turn.rs` do not reach them.
- Persistence is host-driven and best-effort; there is no mid-turn save.
- `Usage` is emitted per step; the variant's doc comment says "per turn".

## Edit statistics

`write`, `edit` and `apply_patch` end a successful result with a
`[+N -M]` trailer (`whycodes_core::edit_trailer`). After each batch the
loop calls `Session::record_edit_results`, which pairs results with their
calls by id and folds the trailers of those three tools into
`Session::edit_stats` (`whycodes_session::EditStats`). `/info` shows the
sum. `apply_patch` counts hunk lines exactly; `edit` counts the whole
replaced block and an overwriting `write` the whole old file, so the total
is flagged `approximate` once either contributed, and also when a counted
result lost its trailer (truncated, spilled, or the call never got a
result). The counts are not stored in the database: `load_from_db`
rebuilds them from the transcript, which is why a failed edit in history
makes a resumed session's figure approximate.

## Related

- [Permission gate](permission-gate.md) — the gates inside `execute_with_permission`.
- [compaction.md](compaction.md), [prompt-cache.md](prompt-cache.md), [retry.md](retry.md), [mcp-lifecycle.md](mcp-lifecycle.md).
- Guide: [Interactive session](../guide.md#interactive-session), [Output formats (headless / CI)](../guide.md#output-formats-headless--ci), [Hooks](../guide.md#hooks), [Agents](../guide.md#agents).
- [architecture.md](../architecture.md) — crate layering.
- [knowhow.md](../knowhow.md) — terminal/event-loop pitfalls (cancel, blocking stdin, title refine).
