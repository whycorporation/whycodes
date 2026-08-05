# Plan — Competitive latency research & whycode roadmap

**Status:** active · **Priority:** P0 (user-facing speed) · **Related:** [plan-perf-hotpath.md](plan-perf-hotpath.md), [plan-perf-context-tui.md](plan-perf-context-tui.md), [KNOWHOW.md](KNOWHOW.md)

## Goal

Match or beat industry coding agents on **time-to-first-token (TTFT)** and
**time-to-useful-result** without sacrificing correctness (permissions, risk
gate, streaming UI).

## Competitor scan (what the fast ones do)

| Technique | Claude Code | OpenCode | Codex / OpenAI | Aider | Cursor | Whycode (before this plan) |
|-----------|:-----------:|:--------:|:--------------:|:-----:|:------:|:--------------------------:|
| Streaming tokens to UI | ✅ | ✅ | ✅ | ⚠️ | ✅ | ✅ |
| Prompt caching (API) | ✅ Anthropic `cache_control` | ⚠️ provider-dep | ✅ / automatic | — | ✅ | ❌ → **done** Anthropic |
| Parallel tool execution | ✅ prompt + runtime | ✅ issue #24764 | ✅ parallel function calls | — | ✅ | ❌ sequential → **done** safe fan-out |
| Stable tool schema order | ✅ | ✅ | ✅ | — | ✅ | ❌ → **done** sort-by-name |
| Shared HTTP / keep-alive | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ → **done** OnceLock client |
| Non-blocking niceties (title, telemetry) | ✅ | ✅ | ✅ | — | ✅ | ❌ await title → **done** async |
| Skip work on trivial chat | — | — | — | — | fast mode | ❌ → **done** trivial title skip |
| Repo map / tree index (offline) | ✅ | ✅ | ⚠️ | ✅ map | ✅ | ❌ |
| Deferred / progressive tools | ✅ defer_loading | ⚠️ | — | small tool surface | modes | ❌ full ~25 tools always |
| Speculative / prefetch reads | — | — | — | — | ⚠️ | ❌ |
| Auto-compact mid-session | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ compact exists; not auto-before-request |
| Fast model routing (simple vs hard) | ⚠️ | Zen models | mini paths | weak model edits | Auto | ❌ single model |
| Semantic / exact response cache | — | — | gateways | — | — | ❌ |
| Connection warm on startup | ✅ | ✅ | ✅ | — | ✅ | ⚠️ catalog fetch only |

Sources (public docs / issues): Anthropic prompt caching + tool-use caching;
OpenAI latency optimization guide (parallelize, stream, smaller models);
OpenCode parallel tools #24764; Aider repository map; Codex parallel tool calling.

## Latency budget (where seconds go)

For a typical multi-step coding turn:

```
[DNS/TLS] ──► [TTFT: prefill system+tools+history] ──► stream text/tools
                ▲                                    │
                │                                    ▼
         cache miss = slow                    tool run (serial N×)
                                                    │
                                                    ▼
                                              next LLM step …
```

**Rules of thumb**

1. **Prefill cost ∝ input tokens.** System + AGENTS.md + full tool JSON + history.
2. **Cache hit** on system+tools can cut multi-step TTFT dramatically (often 50–90% on that prefix).
3. **N independent greps/reads sequential** ≈ N × tool latency; parallel ≈ max(latency).
4. **Second LLM call after the turn** (title refine) must never block the UI — already fixed.

## Shipped in this wave

| Item | Where | Effect |
|------|--------|--------|
| Shared HTTP client + nodelay/keepalive | `llm/client_identity.rs` | Multi-turn TLS reuse |
| Async title refine + trivial skip + 8s cap | `agent/title.rs`, `tui/run.rs` | First-turn UI free |
| Sorted tool definitions | `tools/executor.rs` | Stable cache prefix |
| Anthropic system+last-tool `cache_control` | `llm/anthropic.rs` | Multi-step TTFT |
| Parallel safe tool fan-out | `agent/agent.rs`, `subagent.rs` | Explore steps 2–5× |
| OpenAI `parallel_tool_calls: true` | `llm/openai.rs` | Model emits multi-tool |

## Next backlog (ordered by ROI × effort)

### P0 — next implementation sprint

1. **Auto-compact before each LLM request** when `token_count() > ¾ · context_window`
   - Hook in `run_turn_with_events` before `build_request`.
   - Prevents death spiral of slow prefill on long sessions.

2. **Core tool profile / deferred tools**
   - Default send ~8–12 “hot” tools (`read`, `write`, `edit`, `grep`, `glob`, `list`, `bash`, `todo_*`).
   - Meta-tool `enable_tools` or skill pack loads github/mcp/lsp on demand.
   - Matches Anthropic `defer_loading` spirit; smaller schema → faster TTFT always.

3. **OpenAI-compat prompt cache headers** where supported
   - OpenRouter / some gateways: `cache_control` on system message content parts.
   - xAI / Groq: document what they honor; no-op if unsupported.

4. **Connection warm at TUI ready**
   - Lightweight `GET /v1/models` or HEAD already partially done via catalog — ensure it always fires before first user keystroke.

### P1 — architecture speed

5. **Offline repo map** (Aider-style)
   - Background: file tree + symbol sketch (ctags/tree-sitter light) into a compact string injected once, cache-marked.
   - Reduces “grep wander” rounds (biggest wall-clock killer).

6. **Speculative prefetch**
   - When user pastes a path or `@file`, start `read` before the model returns.
   - Or: on assistant `ToolStart` stream of `read`, begin I/O before JSON args fully closed when path is known early.

7. **Model routing**
   - Config: `models.fast` / `models.default` / `models.heavy`.
   - Heuristic: short chit-chat + no file intent → fast; multi-file edit → default; plan mode → heavy.
   - Huge UX win for “selam” / “ok” without dumbing down hard tasks.

8. **Parallel *with* permission queue**
   - TUI multi-slot permission or batch-approve; then more tools can fan out.

### P2 — advanced / research

9. **Semantic response cache** for identical system+user within TTL (dev loops).
10. **Provider failover with racing** (first token wins) for flaky gateways.
11. **Local small model** for title / classifier / tool-arg repair.
12. **HTTP/2 + regional endpoint** selection; measure p50/p95 TTFT in `WHYCODE_BENCH`.

## Correctness constraints (do not “optimize” away)

- Shell risk gate + permission ask stay **serial**.
- Mutating tools (`write`/`edit`/`apply_patch`/`git_commit`) stay **serial** (same-file races).
- Never block `agent_busy` on title, analytics, or non-critical network.
- Cache breakpoints only on **stable** prefixes (system, tools, not live user draft).

## Measurement

Add / track in JSONL or bench:

| Metric | Definition |
|--------|------------|
| `ttft_ms` | User submit → first `TextDelta` or `ToolStart` |
| `tool_batch_ms` | Start of batch → all tools done |
| `step_ms` | One LLM stream open→close |
| `cache_read_tokens` | From provider `Usage` |
| `worked_ms` | Already in TUI footer (work only) |

Success criteria for P0 complete:

- Multi-step Anthropic sessions show non-zero `cache_read_input_tokens` after step 1.
- Explore-style turns with 3+ greps/reads: tool wall time ≈ max(single), not sum.
- Trivial greeting: no second LLM call; next prompt immediately available.

## Decision log

| Date | Decision |
|------|----------|
| 2026-08-05 | Ship HTTP pool, async title, tool sort (first latency PR). |
| 2026-08-05 | Ship Anthropic cache_control + parallel safe tools + competitor plan. |
| TBD | Core tool profile vs full tools default — product choice; prefer core+expand. |
