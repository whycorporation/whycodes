# Retry, failover and fallback

> Status: Implemented · Updated: 2026-10-16

Two layers decide what happens when an LLM request fails. The transport
(`whycodes-llm`) classifies the error and retries the HTTP open (or a
non-streaming completion) with full-jitter exponential backoff. The turn loop
(`whycodes-agent`) then handles what the transport gave up on: compact once on
context overflow, rotate to the next named credential on a rate limit, switch
to a `fallback_models` entry for the rest of the turn, or fail the turn.
Mid-stream failures are never retried by the transport; the loop retries a
*step* only in three specific cases. Everything below is read from the code;
unconfirmed details say "not verified".

## How it works

### 1. Classification

`whycodes_llm::classify` (`crates/llm/src/error_class.rs:125-135`): if the
error carries a structured `ErrorKind` (providers set it via
`Error::llm_kind` / `http_kind`), that kind wins and `retryable` comes from
`ErrorKind::retryable` (`crates/core/src/error.rs:38-43`). Otherwise
`classify_message` parses the display string (`error_class.rs:138-286`), in
this order:

| Kind | Matches | Retryable | Retry-After |
|---|---|---|---|
| `Cancelled` | "cancelled"/"canceled" | no | — |
| `Auth` | HTTP 401/403, "unauthorized", "forbidden", "invalid api key", "authentication … fail/error/invalid" | **no** | — |
| `RateLimited` | HTTP 429, "rate limit", "too many requests", `rate_limit_exceeded` | yes | header/body value, else 5 s (`:182`) |
| `Timeout` | "timed out", "timeout", "deadline exceeded" | yes | — |
| `Network` | reqwest connect/reset/DNS/TLS strings | yes | — |
| `Server` | HTTP 5xx, `server_error`, `api_error`, "overloaded", "bad gateway", … — but a 4xx status (≠ 429) in the same body wins as `Client` | yes | parsed if present |
| `ContextOverflow` | `context_length_exceeded`, "prompt is too long", "input is too long", "request too large", … (`looks_context_overflow`, `:295-305`) | **no** (never re-send the same payload) | — |
| `Client` | any other 4xx (400 included) | no | — |
| `Unknown` | everything else | no | — |

So 401 and 400 are never retried by any layer. The one exception is outside
the policy: a provider sending an OAuth bearer token routes its POST through
`oauth_refresh::send_with_refresh_retry`, which on a 401 force-refreshes the
stored token and repeats the request exactly once
(`crates/llm/src/oauth_refresh.rs:1-15`).

### 2. Transport retry (`execute_with_policy`)

`crates/llm/src/retry.rs:127-189`, wrapped by `LlmTransport::stream`
(`"stream_open"`, `crates/llm/src/transport.rs:44-59`) and
`LlmTransport::complete` / `complete_uncached` (`"complete"`, `:65-115`,
optional 120 s outer timeout). Defaults (`retry.rs:19-26`, `41-51`):

```
max_retries   = 3          (4 tries total)
initial       = 500 ms
max_backoff   = 20 s
max_elapsed   = 60 s       (wall clock since the first attempt)
full_jitter   = true

backoff(n)    = min(initial * 2^(n-1), max_backoff), n = failed attempts  (retry.rs:66-73)
d             = backoff(n)
if Retry-After = ra:  d = max(d, ra).min(max(max_backoff, ra))              (retry.rs:76-86)
if full_jitter:       d = uniform[0, d]                                      (retry.rs:89-100)
d             = min(d, max_elapsed - elapsed)                                (retry.rs:170-173)
```

Retry happens only while `classified.retryable && retries_used < max_retries
&& elapsed < max_elapsed`; otherwise the original error is returned. Logs:
`llm retrying` / `llm retry giving up` / `llm retry succeeded` with
attempt, kind, status and delay.

Note on the formula: the Retry-After floor is applied *before* jitter, so with
`full_jitter = true` (the default) the actual sleep is uniform in
`[0, max(backoff, ra)]` and can be shorter than Retry-After, despite the
comment at `retry.rs:79`.

The same policy runs for subagent/swarm workers (`crates/agent/src/subagent.rs`
`default_transport().stream`), title refine and memory retain; compaction's
summary call uses a tighter one (2 retries, 200 ms, 3 s, 90 s;
`crates/agent/src/agent/compact.rs:136-142`).

### 3. The turn loop after the transport gives up

`Agent::run_turn_steps`, `crates/agent/src/agent/turn.rs:339-421`. The open is
raced against the cancel flag (`tokio::select! biased`, `:339-358`). On
`Err(e)`:

```
kind = classify(&e).kind
1. ContextOverflow && overflow_retries < 1   -> overflow_retries += 1
                                                compact_session(...)            turn.rs:363-379
                                                continue   (same step, rebuilt request)
2. RateLimited                                -> failover_api_key(provider, current)
                                                if a *next* named credential exists:
                                                  api_key = next; sticky_credential = name
                                                  continue   (same provider/model)      turn.rs:380-399
3. kind.retryable() && next_fallback(from)    -> api_key = target.key
                                                fallback = Some((idx, target, provider))
                                                continue                                 turn.rs:400-418
4. otherwise                                  -> return Err(e)                          turn.rs:419
```

`continue` re-enters the loop top: background completions and steering are
drained again, `turn_count` increments, caps/shake/autocompact run, and the
request is rebuilt from the session (`turn.rs:180-277`). There is no sleep on
these loop-level retries; the transport already slept.

**Credential rotation** (`Agent::failover_api_key`,
`crates/agent/src/agent/mod.rs:785-815`): the provider's
`[providers.<id>.credentials]` names in lane order (`names_for_lane`,
`crates/core/src/types.rs:483-507`: the other lane's names are kept last when
`reserve > 0`), resolved to env secrets, the current key located, and the
*next* non-empty one returned. Same model, same provider. When none is left,
step 3 applies. The chosen name is pinned in `sticky_credential`; a successful
open with no sticky name records `"default"` (`turn.rs:427-429`).

**Fallback models** (`Agent::next_fallback`, `mod.rs:901-944`): scans
`session.fallback_models` from index `from` (= previous fallback index + 1,
`turn.rs:400`), skipping malformed `provider/model` entries, the active
target, unregistered providers and entries without a key (same provider →
active key; else config `api_key`, else `<PROVIDER>_API_KEY`). The `fallback`
local is read at the top of every subsequent step (`turn.rs:176-184`), so the
switch **stays for the rest of the user turn**: later steps, tool execution
(`turn.rs:791-799`), compaction (`:372`) and memory retain (`:918-927`) all use
the fallback provider/model/key. The next `run_turn_steps` call starts on the
primary again.

**Context overflow** is handled before the retryable check because it is not
retryable; it is compact-and-retry (see [compaction.md](compaction.md)) with a
budget of one per user turn.

### 4. Step retries inside the stream body

Three paths discard the current draft and run the step again (`turn.rs:445-628`):

- Mid-stream `ContextOverflow` (stream `Err` item `:472-494`, or in-band
  `StreamEvent::Error` `:597-619`), once per turn: compact, `stream_rule_retry
  = true`, `break`, `continue`.
- Stream rule hit (`first_stream_rule_hit` on the accumulated text,
  `:520-533`; `apply_stream_rule_hit`, `:1015-1034`): a user message
  `<whycodes_rule name=…>hint… The previous draft was discarded…</whycodes_rule>`
  is **appended to the session**, then `break`, `continue`. No cap on how many
  times this can fire in a turn.
- Harmony protocol leak detected after the stream ends (`harmony_leak`,
  `:634-656`): draft discarded, up to 2 retries, then `Err`.

Any other mid-stream error returns `Err` immediately (`:495-509`) — no
transport retry, no fallback. Cancellation persists what was streamed so far
(`:448-461`) and returns `Cancelled`.

### 5. Autocompact circuit breaker

Independent of errors: after 3 consecutive compaction passes that leave the
session over `compaction_threshold`, `compact_paused` stops further automatic
passes for the rest of the turn (`turn.rs:247-266`,
`MAX_CONSECUTIVE_COMPACT_FAILURES`, `mod.rs:158`). Overflow compaction still
runs; it does not consult the breaker.

### 6. Not a retry: the first-token race

`session.model_race` opens a partner stream when the primary has produced no
token after `race_after_ms` and keeps whichever emits first
(`crates/llm/src/race.rs:1-6`, `transport.rs:118-147`). It is opt-in and
orthogonal to retries; the loser is cancelled, not retried.

### Audit: replay after side effects

Question: can a step be retried or replayed after the assistant already
streamed visible text, or after a tool call executed in that step?

**Order of appends in one step** (`turn.rs`): nothing touches the session
between request build (`:275`) and stream end except the three retry paths
above. The assistant message (thinking + text + tool_use blocks) is appended
only after the stream closes and the harmony check passes (`:696-717`);
tools run after that (`:791-804`); results are appended at `:858`.

1. **Retry at open** (transport retries, overflow-at-open, credential
   rotation, fallback): no token was streamed, nothing was appended, no tool
   ran. The request is rebuilt from the session (`:275`), so a compaction or a
   steering message between attempts changes the payload. Replay is safe.
   Caveat: `turn_count` was already incremented (`:200`), so on the retried
   first step the intent posture and magic-keyword notice (gated on
   `turn_count == 1`, `:291-313`) are **not** re-applied, and `max_turns`
   counts the retry.
2. **After visible text streamed**: yes, for the three paths in §4.
   `accumulated_text` is a local created at `:315`; the retry paths `break`
   and `continue` before `:696-703`, so the partial text is **discarded** —
   never appended to the session, never added to `final_text`. The UI has
   already received `TurnEvent::TextDelta` for it (`:518`); nothing retracts
   those events, so the viewer may see the discarded draft followed by the
   retried answer (TUI merge behaviour not verified). The model never sees
   its discarded draft, except in the stream-rule case where the appended
   `<whycodes_rule>` user message (`:1028-1032`) tells it one existed.
   Side effects before the retry: speculative `read` I/O started while the
   tool-call JSON was still streaming (`maybe_start_speculative`, `:541`,
   `:549`); all retry paths call `speculative_read::abort_all` first and the
   reads are read-only.
3. **After a tool call executed**: no. Tool calls exist only after
   `assembler.finish()` (`:631`), the assistant turn is persisted before
   execution (`:716`), and no code path loops back to re-open the *same* step
   once `execute_tool_calls` has run. If execution itself errors, every call
   gets a `Tool not run…` result and the turn ends (`close_unanswered_calls`,
   `:807`, `:1115-1130`). The following iteration is a new step with the tool
   results in context, not a replay. A rate limit or 5xx on that next step
   therefore cannot cause a tool to run twice; the fallback switch happens at
   the open of the next step, after results were appended.

Answer: a step can be retried after visible text was streamed (text dropped,
session untouched except the stream-rule note), and never after a tool call
of that step executed.

### 7. Absent today (facts, not plans)

- No usage-aware preflight: nothing compares the request's token estimate or
  the provider's reported `input_tokens` with the model's context window
  before sending; the only pre-send guard is the chars/4
  `compaction_threshold`.
- No cooldown-based revert to the primary model: `fallback` lives for one
  `run_turn_steps` call; there is no health record, timer or probe across
  turns. Every new user turn starts on the primary and may fail over again.
- No retry of mid-stream drops (no resume, no re-open with the partial text).
- No cross-step retry budget: each open gets the full policy again.
- No proactive rate limiting in the agent path: `RateLimiter`
  (`crates/llm/src/rate_limit.rs:396-480`) exists but has no caller outside
  its own module and tests (grep over `crates/`).
- `FallbackChain::complete` (`crates/llm/src/fallback.rs`) is a separate
  non-streaming helper with no retry or classification; the agent path does not
  use it.

## Configuration and overrides

| knob | where | effect |
|---|---|---|
| `fallback_models = ["provider/model", …]` | `[session]`, `crates/config/src/types.rs:1373-1378`; `mod.rs:637` | ordered fallback targets after retries; needs a key per entry |
| `[providers.<id>.credentials] order / env / reserve` | config → `ProviderCredentials` (`crates/core/src/types.rs:420-432`); `mod.rs:680-683` | named keys for 429 rotation on the same model; `reserve > 0` keeps the other lane's key last |
| `CI=true` / `WHYCODES_CREDENTIAL_LANE=ci` | env (`process_is_ci`, used at `mod.rs:791`) | selects the lane order |
| `[providers.<id>] api_key`, `<PROVIDER>_API_KEY` | config / env | keys for cross-provider fallback entries |
| `compaction_threshold`, `compaction_llm` | `[session]` | govern the compact-and-retry outcome and the breaker |
| `[[session.stream_rules]] name / pattern / hint` | `[session]`, compiled at `mod.rs:661`, `compile_stream_rules` `mod.rs:247-265` | regex abort-and-retry of a draft |
| `model_race`, `race_after_ms` (default `off`, 800) | `[session]`, `types.rs:1364-1372` | first-token race (not a retry) |
| `whycodes auth login <provider>` | CLI | registers an OAuth source so a 401 may refresh once |

No configuration changes the retry counts, delays or elapsed cap: callers
construct `RetryPolicy` in code (`transport.rs:27-35`, `compact.rs:136-142`).
No env var overrides them (grep `WHYCODES_` in `crates/llm`, `crates/agent`).

## Implementation files

- `crates/core/src/error.rs` — `ErrorKind`, `ErrorKind::retryable`, `transport_kind`.
- `crates/llm/src/error_class.rs` — `classify`, `classify_message`, `user_message`, status/Retry-After extraction.
- `crates/llm/src/retry.rs` — `RetryPolicy`, `backoff_for_attempt`, `sleep_duration`, `execute_with_policy`.
- `crates/llm/src/rate_limit.rs` — `parse_retry_after` (integer seconds or RFC 2822 date, default 5 s), unused `RateLimiter`.
- `crates/llm/src/transport.rs` — `LlmTransport::{stream, complete, complete_uncached, stream_turn}`, `format_turn_error`.
- `crates/llm/src/oauth_refresh.rs` — one-shot 401 refresh for OAuth credentials.
- `crates/llm/src/race.rs` — first-token race.
- `crates/llm/src/fallback.rs` — `FallbackChain` (non-streaming, not on the agent path).
- `crates/agent/src/agent/turn.rs` — open retry decisions (`359-421`), stream-body retries (`445-656`), appends (`696-717`, `858`), `close_unanswered_calls`.
- `crates/agent/src/agent/mod.rs` — `failover_api_key`, `next_fallback`, `FallbackTarget`, `sticky_credential`, stream-rule compilation.
- `crates/agent/src/agent/turn_tests.rs` — `rate_limit_retries_same_model_on_next_credential`, `rate_limit_without_failover_returns_err`, `overflow_fail_open_retries_then_succeeds`, `overflow_stream_error_event_retries`, `mid_stream_non_overflow_error_fails_turn`, `stream_rule_interrupts_draft`, `harmony_leak_retries_then_errors`, `cancel_during_stream_persists_accumulated_text`.
- `crates/llm/src/retry_tests.rs`, `error_class_tests.rs` — policy and classification tests.

## Known limitations / not enforced

- Jitter can undercut Retry-After (§2); a provider that enforces the header
  strictly may answer 429 again, consuming another attempt.
- Classification of unstructured errors is string matching; a 4xx body that
  mentions "overloaded" is forced to `Client` (`error_class.rs:239-246`), a
  body that mentions "timeout" anywhere becomes retryable.
- Credential rotation only applies to a `RateLimited` open; a 5xx goes
  straight to `fallback_models` without trying the other credentials.
- The sticky credential is per `Agent`, not per session; it is not reset when
  the turn ends.
- A fallback entry on the same provider reuses the *current* key, which may be
  the one that was just rate-limited.
- Stream-rule retries are unbounded; a rule whose hint keeps matching its own
  pattern can loop until `max_turns` (headless) or cancel (TUI).
- The retried first step loses the intent/magic suffix (audit §1).
- `format_turn_error` is what the UI shows; the raw body is only in logs.
- Subagents have no loop-level handling at all: no overflow compaction, no
  credential rotation, no fallback (`crates/agent/src/subagent.rs:387-389`).

## Related

- [`../guide.md#configuration`](../guide.md#configuration) — named credentials, `fallback_models`, "Context-window errors", stream rules.
- [`../guide.md#slash-commands`](../guide.md#slash-commands) — `/fresh`, `/cost` credential name.
- [turn-loop.md](turn-loop.md) — §6 "Opening the stream", §7 "Stream body".
- [compaction.md](compaction.md) — what compact-and-retry does to the transcript.
- [prompt-cache.md](prompt-cache.md) — a retried request is byte-identical unless the session changed.
- [permission-gate.md](permission-gate.md), [mcp-lifecycle.md](mcp-lifecycle.md).
- [`../knowhow.md`](../knowhow.md) — `/v1/models` context window, `max_tokens` vs `context_window`.
