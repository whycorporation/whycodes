# Plan — Competitive latency research & whycode roadmap

**Status:** P0+P1 done · P2 partial (LLM compact quality + speculative early `read` shipped) · **Priority:** P0 (core complete) · **Last review:** 2026-08-13  
**Related:** [archive/plan-perf-hotpath.md](archive/plan-perf-hotpath.md), [archive/plan-perf-context-tui.md](archive/plan-perf-context-tui.md), [comparison.md](comparison.md), [FEATURES.md](FEATURES.md), [KNOWHOW.md](KNOWHOW.md)  
**Primary peers for this plan:** **OpenCode** (anomalyco, local tree `/tmp/opencode-src` @ shallow tip), **jcode** (binary `v0.64.2` + public issues; no full source in-tree)

## Goal

Match or beat peer coding agents on **time-to-first-token (TTFT)** and
**time-to-useful-result**, without sacrificing correctness (permissions, shell
risk, streaming UI).

This revision **re-checks** the earlier plan against real OpenCode source and
jcode architecture signals. Several backlog rows were wrong or stale.

---

## Latency budget (unchanged physics)

```
[DNS/TLS] ──► [TTFT: prefill system+tools+history] ──► stream text/tools
                ▲                                    │
         cache miss = slow                    tool batch (serial Σ vs parallel max)
                                                    │
                                                    ▼
                                              next LLM step (× N agent steps)
```

1. Prefill cost ∝ **stable prefix** (system, tools) + **growing suffix** (history, tool dumps).
2. Intra-turn multi-step loops only win on cache if breakpoints sit on a **stable** boundary (OpenCode: last tool + system + **latest user message**).
3. Independent tools sequential ⇒ wall ≈ sum; parallel ⇒ wall ≈ max.
4. Niceties (title, telemetry, animation) must never hold `agent_busy`.

---

## Deep dive: OpenCode

**Sources:** shallow clone of [anomalyco/opencode](https://github.com/anomalyco/opencode) (`packages/opencode`, `packages/llm`), public perf issues (#24764 parallel tools, #20285 session loop, #14195 / #29638 subtask serial, #29819 parallel subtasks PR).

### Architecture (latency-relevant)

| Piece | What they do | Why it matters |
|-------|----------------|----------------|
| **Client / server** | Long-lived server + TUI/desktop/web clients | Keeps model catalog, MCP, sessions warm across reconnects |
| **`packages/llm` + `cache-policy.ts`** | Default `cache: "auto"` injects Anthropic/Bedrock breakpoints | Production-grade prompt cache |
| **`SessionCompaction`** | Overflow detect → dedicated compaction agent + **LLM summary** + prune tool outputs | Context stays usable; smarter than drop-only |
| **`doom_loop` permission** | Same tool + same args N times → ask user | Stops silent infinite tool loops (wall-clock killer) |
| **Tool surface** | Core set similar to whycode (`read`/`write`/`edit`/`grep`/`glob`/`bash`/`task`/`lsp`/…) | Same order of schema cost |
| **Shell prompt** | Explicitly tells model to emit **multiple bash calls in one message** when independent | Encourages parallel *intent* |
| **Instruction load** | `Effect.forEach(..., { concurrency: 8 })` for AGENTS/instruction files | Parallel I/O at session build |
| **Subagents / task** | Task tool can spawn sessions; historically **pop one-at-a-time** (issues #14195, #29638) | Parallel *agents* still a weak spot |

### OpenCode prompt-cache policy (copy-worthy)

From `packages/llm/src/cache-policy.ts` (verbatim intent):

- **Default is on** (`undefined` → `"auto"`).
- Auto markers:
  1. **Last tool definition** (`cache_control` on last tool)
  2. **Last system part**
  3. **Latest user message** (last text/content part)
- Protocols that ignore inline hints (OpenAI implicit, Gemini) skip the pass.
- Rationale: during one user turn the agent does many assistant/tool round-trips; caching at the **user message** boundary makes every *intra-turn* API call a cache hit on the prefix.

**Whycode vs OpenCode (cache):** system + last tool + **latest user message** are marked when `session.prompt_cache = "auto"` (default). `crates/llm/src/cache.rs` (`CachePolicy::Auto`) is wired into Anthropic (and compatible) request building via `LlmRequest.use_prompt_cache`.

### OpenCode parallel tools — status nuance

- Issue **#24764** frames parallel tool execution as still needed (~30–50% tool-heavy latency).
- Shell *prompt* asks for parallel bash calls; runtime fan-out is **not** clearly universal for all tools (tool-runtime `dispatch` is one-call).
- Subtasks often still serial (queue / `pop`).

**Implication:** whycode’s **safe parallel fan-out** (read-class tools) is already competitive; do not assume OpenCode is “done” here. Keep mutators/shell serial (they do).

### OpenCode session-loop perf ideas (#20285 class)

Issue themes (not all merged; treat as backlog inspiration):

- Message / tool **definition memoization** (rebuild less per step)
- Doom-loop early stop
- Summary **debounce** (don’t compact thrash)
- Parallel plugin events

### OpenCode → whycode takeaways

| Take | Action |
|------|--------|
| Cache `auto` = tools + system + **latest user** | **P0** wire into Anthropic body builder |
| Compaction agent + prune tool dumps | P1: optional LLM summary compact (we only drop+stub) |
| Doom-loop gate | P0/P1: detect N identical tool calls → refuse or ask |
| Warm server process | P2: optional `whycode serve` long-lived (exists partially) |
| Parallel subagents | Keep phase-7 drop unless product needs swarm |

---

## Deep dive: jcode

**Sources:** local binary `jcode` **v0.64.2** (`2026-07-30`), `~/.jcode/config.toml` (features: `memory`, `swarm`), public GitHub issues dump (`/tmp/opencode/jcode_*.json`), whycode docs ([comparison.md](comparison.md), [FEATURES.md](FEATURES.md), [archive/phase-7](archive/phase-7-multi-agent.md)).

### Architecture (latency-relevant)

| Piece | Evidence | Latency angle |
|-------|----------|----------------|
| **Always-on shared server** | Issues: first launch spawns fixed-socket daemon; `jcode serve` / `connect` | Process + connection warm; multi-client share MCP/catalog |
| **Swarm** | `features.swarm = true`, swarm panel keybind, many swarm bugs | Wall-clock on *wide* tasks via multi-agent, not single-turn TTFT |
| **Memory + embeddings** | `features.memory = true`; ONNX `all-MiniLM` called out in comparison.md | Cross-session recall vs RAM cost (claimed 27.8 MB RSS with embeddings off) |
| **agentgrep** | Dedicated in-process search; display flag `show_agentgrep_output` | Fewer shell-outs to `rg`; faster local search |
| **Compaction family** | Issues: `hard_compact`, emergency compact loops, OpenAI encrypted compaction state | Aggressive context control under load — also a bug surface |
| **OAuth providers** | Claude / ChatGPT / Copilot paths | Product friction; not pure latency |
| **Curated Claude tools on OAuth** | Issue: Anthropic OAuth route forces curated Claude Code tool set | Smaller/stable tool schema → better cache + TTFT on that route |
| **MCP `locked_tools` race** | Tools missing until async register finishes | Cold-start tool set instability |
| **Effort / model switch hotkeys** | config keybindings | Fast human routing between heavy/light |

### jcode claims we treat carefully

| Claim | Treatment |
|-------|-----------|
| 14 ms TTFT frame / 27.8 MB RSS | **Unverified** (comparison.md). Our release floor is different metric family (CLI/`--version`, TUI first paint). |
| Swarm “faster” | Only when work decomposes; whycode phase-7 **dropped** for same reason on ~25k LOC projects. |
| Memory always on | Latency + correctness tradeoff; embeddings add load. |

### jcode pain → whycode avoid-list

From open issues (latency/correctness adjacency):

- Compaction that **doesn’t shrink** or **loops emergency compact**
- MCP tools **not registered** when first LLM call fires
- Streaming without terminal **usage** chunk (meter/compact blind)
- Swarm spawn mid-turn races / ignored `swarm_model`
- Model list / context window **wrong** → wrong compact budget

### jcode → whycode takeaways

| Take | Action |
|------|--------|
| In-process search (agentgrep spirit) | Already: Rust `grep`/`glob` tools; keep **no shell dependency** |
| Warm daemon optional | P2: document/strengthen `serve` for multi-session |
| Curated tool set on heavy providers | **P0** core tool profile |
| Memory | Separate plan ([archive/plan-memory.md](archive/plan-memory.md)); not TTFT P0 |
| Swarm | Stay **out** of latency P0; revisit only for huge monorepos |
| Effort / fast model switch | **P0/P1** model routing + keybind |

---

## Comparison matrix (revised)

| Technique | Claude Code | OpenCode | jcode | Codex | Whycode now |
|-----------|:-----------:|:--------:|:-----:|:-----:|:-----------:|
| Streaming UI | ✅ | ✅ | ✅ | ✅ | ✅ |
| Prompt cache system+tools | ✅ | ✅ `auto` | ⚠️ OAuth curated | ✅/implicit | ✅ Anthropic partial |
| Cache **latest user msg** | ✅ | ✅ **auto** | ⚠️ | ⚠️ | ✅ |
| Parallel safe tools | ✅ | ⚠️ issue #24764 | ⚠️ | ✅ | ✅ + perm queue |
| Parallel subagents / swarm | ⚠️ teams | ⚠️ serial tasks | ✅★ swarm | ⚠️ | ❌ (dropped) |
| Shared HTTP / keep-alive | ✅ | ✅ | ✅ server | ✅ | ✅ OnceLock |
| Non-blocking title/telemetry | ✅ | ✅ | ✅ | ✅ | ✅ async title |
| Auto context compact | ✅ | ✅ LLM+prune | ✅ hard/soft (buggy edge) | ✅ | ✅ drop + **old-tool prune 2k** |
| Doom-loop guard | ✅ | ✅ permission | ⚠️ | ⚠️ | ✅ 3× refuse |
| Core / deferred tools | ✅ | ⚠️ | ⚠️ curated OAuth | — | ✅ `tool_profile=core` |
| Repo map / memory index | ✅ | ⚠️ | ✅ memory+embed | ⚠️ | ❌ P2 |
| Model routing / effort | ⚠️ | Zen models | ✅ effort keys | mini | ✅ trivial→fast |
| Warm long-lived process | ⚠️ | ✅ server | ✅★ daemon | ⚠️ | ⚠️ catalog + pool |

---

## Shipped already (do not re-list as “next”)

| Item | Commit era | Notes |
|------|------------|--------|
| Shared HTTP client | `b2ffb3c` | OnceLock, nodelay, keepalive |
| Async title + trivial skip + 8s | `b2ffb3c` | UI not blocked |
| Sorted tool definitions | `b2ffb3c` | Stable cache prefix |
| Anthropic system + last-tool cache | `9d1f62c` | partial |
| Parallel safe tools | `9d1f62c` | Reads only; shell/mutators serial |
| OpenAI-compat `parallel_tool_calls` | `9d1f62c` | Encourage multi-tool emit |
| Auto-compact before LLM step | `9d1f62c` | Heuristic drop at `compaction_threshold` |
| **OpenCode-parity cache** (system+tools+**latest user**) | P0.1 | `llm/cache.rs` `apply_anthropic_cache_policy` |
| **Doom-loop** (3× same tool+args) | P0.2 | refuse + error tool result |
| **Core tool profile** (default) | P0.3 | `session.tool_profile = "core"\|"full"` |
| **JSONL metrics** | P0.4 | `turn.step` / `turn.done` (`ttft_ms`, `step_ms`, `tool_batch_ms`, cache tokens) |
| **Old-tool prune (2k)** | P1 | OpenCode-style shrink of older tool dumps in compact |
| **Model routing** | P1 | trivial chat → `model_fast` or small sibling |
| **Permission queue** | P1 | VecDeque multi-ask; Ask tools can parallelize |
| **@file cap** | P1 | 24k chars per inlined file |
| **prompt_cache wire** | P1 | `session.prompt_cache=none` disables markers |

### Config knobs

```toml
[session]
tool_profile = "core"   # or "full"
prompt_cache = "auto"   # or "none"
model_fast = "anthropic/claude-haiku-4-5-20251001"  # optional
compaction_threshold = 150000
```

---

## Remaining backlog

### P0 + P1 core — complete

### P2 — optional product scale

1. LLM-summary compact agent — **improved** (local summary includes goals/paths; LLM uses *dropped* transcript; runs when messages were dropped, not only when still over budget)  
2. Speculative stream-arg early `read` — **shipped** (`crates/agent/src/speculative_read.rs`; path closes mid-stream → I/O overlaps remaining tokens)  
3. Long-lived daemon multi-session warm  
4. Cross-session memory ([archive/plan-memory.md](archive/plan-memory.md))  
5. Swarm — monorepo only (phase-7 still holds)  
6. First-token race failover / semantic response cache

Remaining P2 items are product-scale (daemon, swarm, semantic cache), not TTFT core.

---

## Correctness constraints (keep)

- Shell risk + permission **serial**.  
- Mutators (`write`/`edit`/`apply_patch`/`git_commit`) **serial**.  
- Never block `agent_busy` on title/analytics.  
- Cache breakpoints only on **stable** prefixes (not live incomplete draft).  
- Compaction must **not** thrash (jcode emergency-compact bug class).

---

## Measurement & acceptance

| Metric | Definition | Target |
|--------|------------|--------|
| `ttft_ms` | submit → first TextDelta/ToolStart | Track p50/p95; no absolute claim yet |
| `tool_batch_ms` | batch start → all tools done | 3× parallel reads ≈ max(single) |
| `cache_read_tokens` | provider Usage | >0 on Anthropic step ≥2 of same turn |
| `worked_ms` | footer | excludes title refine |
| Doom-loop | N identical tools | stopped ≤ N+1 without user Esc |

---

## Decision log

| Date | Decision |
|------|----------|
| 2026-08-05 | Ship HTTP pool, async title, tool sort. |
| 2026-08-05 | Ship Anthropic system+tool cache, parallel safe tools, first plan draft. |
| 2026-08-05 | **Revise plan:** deep OpenCode (`cache-policy`, doom_loop, compaction) + jcode (daemon, swarm, memory, agentgrep, compaction bugs). Mark auto-compact **shipped**. Elevate **latest-user cache** + doom-loop + core tools to P0. Swarm stays non-P0. |
| 2026-08-05 | **Implement P0.1–P0.4:** OpenCode cache policy, doom-loop, core tools default, JSONL latency metrics. |
| 2026-08-05 | **Implement P1:** tool prune 2k, model routing, perm queue, @file cap, prompt_cache wire. |
| TBD | LLM-summary compact only if prune+drop insufficient in production metrics. |

---

## Appendix A — OpenCode file map (for implementers)

| Path | Topic |
|------|--------|
| `packages/llm/src/cache-policy.ts` | Auto cache breakpoints |
| `packages/llm/test/cache-policy.test.ts` | Expected Anthropic body shape |
| `packages/llm/src/protocols/anthropic-messages.ts` | ≤4 breakpoints, wire format |
| `packages/opencode/src/session/compaction.ts` | LLM compact + prune |
| `packages/opencode/src/session/overflow.ts` | When to compact |
| `packages/opencode/src/session/processor.ts` | `doom_loop` permission |
| `packages/opencode/src/tool/shell/prompt.ts` | Parallel bash instruction |
| Issues #24764, #20285, #14195, #29638 | Perf / parallel backlog |

## Appendix B — jcode signals (for implementers)

| Signal | Where |
|--------|--------|
| Binary version | `jcode version` → v0.64.2 |
| Features | `~/.jcode/config.toml` `[features] memory/swarm` |
| Architecture notes | [comparison.md](comparison.md), [FEATURES.md](FEATURES.md) |
| Swarm decision | [archive/phase-7-multi-agent.md](archive/phase-7-multi-agent.md) dropped |
| Issue themes | compaction loops, MCP lock race, daemon, swarm_model ignore |
