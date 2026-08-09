# Roadmap status

Living tracker for open work and past decisions. Update this file in the same
commit as the work it describes.

Last updated: **2026-08-09** (shipped plans archived)

## Legend

| Status | Meaning |
|---|---|
| `not started` | No work has begun |
| `in progress` | Some tasks done, acceptance criteria not all met |
| `implemented` | Code shipped; residual criteria need an external step (e.g. first release) |
| `blocked` | Waiting on a decision or external dependency |
| `done` | Acceptance criteria verified |
| `dropped` | Deliberately abandoned; rationale kept in archive |
| `deferred` | Explicitly postponed (e.g. post product launch) |

## Open plans

| Plan | Doc | Status | Notes |
|---|---|---|---|
| Performance residual | [plan-performance.md](plan-performance.md) | **mostly done+** | Subagent usage fold + bench ceilings shipped. Live provider reconcile still manual. |
| Distribution & self-update | [plan-distribution.md](plan-distribution.md) | implemented · **last** | Assets live. Remaining: tagged release, Homebrew binary formula, install smoke test. |
| OAuth & credential discovery | [plan-oauth.md](plan-oauth.md) | **partially shipped** | `whycode auth login` for anthropic/openai/github-copilot/google; calls routed for anthropic + copilot ([auth.md](auth.md)). Credential discovery still open. |
| Latency competitors | [plan-latency-competitors.md](plan-latency-competitors.md) | P0+P1 done | Cache, parallel tools, core profile, routing, doom-loop; P2 optional. |
| System optimization 2026-08 | [plan-optimize-2026-08.md](plan-optimize-2026-08.md) | Session A done | Session B open: deferred MCP/auto-index, closed-message markdown cache, `LlmRequest` Arc, token-estimate cache. |

## Shipped (archived)

| Phase | Archive | Status |
|---|---|---|
| 1 Shell command risk | [archive/phase-1-command-risk.md](archive/phase-1-command-risk.md) | done |
| 4 CI quality budgets | [archive/phase-4-ci-budgets.md](archive/phase-4-ci-budgets.md) | done |
| 7 Multi-agent coordination | [archive/phase-7-multi-agent.md](archive/phase-7-multi-agent.md) | **shipped lightweight (2026-08-07)** — `swarm` + git worktrees + 3-way merge + file claims / toast |
| 8 TUI rendering | [archive/phase-8-tui.md](archive/phase-8-tui.md) | done |
| 9 Shell OS sandbox | [archive/phase-9-sandbox.md](archive/phase-9-sandbox.md) | done |
| Context + TUI paint | [archive/plan-perf-context-tui.md](archive/plan-perf-context-tui.md) | done — compact, tool cap, height cache, dirty-draw, stream coalesce |
| Perf hot path | [archive/plan-perf-hotpath.md](archive/plan-perf-hotpath.md) | done — release profile, FxHash keys, BPE cache, needle fast path |
| FEATURES accuracy | [archive/plan-features-improvements.md](archive/plan-features-improvements.md) | done — matrix fixed; `/tools` `/info` surface latency knobs |
| CC research A1–A7 | [archive/plan-cc-a1-a7.md](archive/plan-cc-a1-a7.md) | shipped 2026-08-07 — PromptCommands, /context, LLM compact, path globs, mcp serve, image read, idle suggestions |
| Parallel multi-session | [archive/plan-parallel-multi-session.md](archive/plan-parallel-multi-session.md) | shipped S1–S6 — dashboard (Ctrl+O), Ctrl+N, Ctrl+Tab, per-runtime DB |
| Semantic memory | [archive/plan-memory.md](archive/plan-memory.md) | shipped v1+v2 — retain, sync, code RAG, subagent banks, optional ONNX |

Index of archives: [archive/README.md](archive/README.md).

## Deferred (post product launch)

| Item | Status | Notes |
|---|---|---|
| ACP — Agent Client Protocol | deferred | Editor ↔ agent (JSON-RPC). `whycode acp` stub only. Not agent-to-agent. |
| `web` surface | stub | Same band as ACP; not blocking launch. |

## Current focus

Priority (owner: **public install / repo visibility last**):

1. **Performance residual** — subagent usage fold + CI bench ceilings shipped ([plan-performance.md](plan-performance.md)). Live provider reconcile optional/manual.
2. **Plugins depth** — `plugins.toml` → `plugin_*` tools; `whycode plugins list`; project+global load. Marketplace still out of scope.
3. **Latency P2 (optional)** — residual rows in [plan-latency-competitors.md](plan-latency-competitors.md).
4. **OAuth** — subscription login shipped (2026-08-09); credential discovery + openai/google call routing open.
5. **ACP / web** — deferred post product launch.
6. **Public release (last)** — repo public, Homebrew binary formula, Windows install smoke. Assets already cut as `v0.1.0`.

## Decision log

| Date | Decision | Rationale |
|---|---|---|
| 2026-07-31 | Stop targeting "OpenCode parity" as the project's goal | Parity is definitionally a following position. See [comparison.md](comparison.md). |
| 2026-07-31 | Re-implement borrowed designs rather than vendoring jcode source | Port the design; keep jcode copyright on any derivative file. |
| 2026-07-31 | Safety before features | Shell risk classification before more user-facing work. |
| 2026-07-31 | Default `bash_risk_threshold` is `destructive`, not `caution` | `caution` prompts on normal cleanup and gets turned off. |
| 2026-07-31 | Unresolvable targets escalate to `destructive`, never `catastrophic` | Unknown ≠ known-bad; catastrophic is unpromptable. |
| 2026-07-31 | Unrecognised commands are `safe` | Unknown-means-dangerous prompts on every build script. |
| 2026-07-31 | Added TUI phase targeting opencode's look and feel | TUI is the product surface; theme JSON schema transfers, code does not. |
| 2026-08-04 | Shell OS sandbox defaults to `workspace`, network on, fallback allow | Second lock beyond string risk; keep cargo/npm/git; don't break non-Linux. |
| 2026-08-04 | **ACP deferred until after product launch** | Ship terminal product first. Real ACP is post-release IDE surface work. |
| 2026-08-04 | Archive completed phase docs; keep only open plans live | Numbered 1–9 cluttered the tree; done work lives under `docs/archive/`. |
| 2026-08-04 | Network allowlist for HTTP tools (`webfetch` / search / GitHub) | Domain gate on agent egress; shell stays `sandbox_network` binary (no proxy). |
| 2026-08-04 | Cut `v0.1.0` first release assets | Four targets + `SHA256SUMS`. Repo remains private until owner opens it for anonymous install. |
| 2026-08-04 | Persist session token usage + real `stats` | Provider-reported totals in SQLite; no more message×500 estimate. |
| 2026-08-04 | Config pre/post tool hooks | Shell hooks around tool calls; `block_on_failure` on pre only. Marketplace later. |
| 2026-08-05 | Latency P0/P1 shipped | OpenCode-parity cache, core tools, routing, doom-loop, prune; see plan-latency-competitors. |
| 2026-08-05 | FEATURES.md rewritten | Mouse/resume/latency rows fixed; no stale ❌ for shipped TUI. |
| 2026-08-07 | Swarm + conflict notify (lightweight) | `swarm` tool, shared `FileClaimRegistry`, write/edit/apply_patch gate, TUI toast. |
| 2026-08-07 | Swarm git worktrees | Detached worktrees under `.whycode/swarm/`, three-way merge into main, force-remove on finish; config `swarm.worktrees`. |
| 2026-08-07 | Background + schedule (Claude Code–inspired) | Process-local bg jobs (`bash background=true`, `bg`, `/bg`), delayed `schedule` + `/loop`, risk-gated; no persistent cron. |
| 2026-08-07 | Process-sub + dynamic interpreter risk | `<(…)` `>(…)` `=(…)` and `python -c "$(…)"` → Destructive prompt. |
| 2026-08-07 | `/doctor` | Env/key/sandbox/git/bg diagnostics (CC doctor idea). |
| 2026-08-07 | Claude Code feature study | Ideas-only notes; no vendored code. |
| 2026-08-07 | FEATURES §11 automation | Background shell jobs (`background: true`, `bg`, toast, `/bg`), `schedule` + `/loop` queue; cloud still deferred. |
| 2026-08-07 | CC inspiration phases P1–P3 | Autocompact breaker; `tool_search` deferred tools; `bash(git *)` rules + dangerous Allow→Ask; `/diff` `/cost`; `worktree` enter/exit. |
| 2026-08-07 | Claude Code research report | Architecture / inventory / gaps analysis (kept internal). |
| 2026-08-07 | A1–A7 → roadmap | [archive/plan-cc-a1-a7.md](archive/plan-cc-a1-a7.md); order A1,A4,A2,A3,A5,A6,A7. |
| 2026-08-07 | A1–A7 shipped | PromptCommands, /context, LLM compact, path globs, mcp serve, image read, idle suggestions. |
| 2026-08-07 | Public release last | Coding/perf/plugins ranked ahead of repo-public + install packaging. |
| 2026-08-07 | Perf residual + plugins | Subagent usage fold into parent; bench-results + CI ceilings; shell plugins as tools. |

## Verification commands

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Also (CI budgets):

```bash
python scripts/check_panic_budget.py
python scripts/check_swallowed_error_budget.py
python scripts/check_dependency_boundaries.py
```
