# Roadmap status

Living tracker for open work and past decisions. Update this file in the same
commit as the work it describes.

Last updated: **2026-08-07** (swarm + conflict notify; latency P0/P1 + FEATURES; `v0.1.0` public install still gated on repo visibility)

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
| Distribution & self-update | [plan-distribution.md](plan-distribution.md) | implemented | `v0.1.0` assets live. **Public install** needs repo visibility=public (or `GITHUB_TOKEN`). Homebrew formula still HEAD-only until `update_homebrew_formula.sh`. |
| OAuth & credential discovery | [plan-oauth.md](plan-oauth.md) | blocked | Owner: client registration + provider terms before any code. |
| Performance residual | [plan-performance.md](plan-performance.md) | mostly done | Harness + usage persistence + `whycode stats`. Open: subagent tokens, provider reconcile, optional CI ceilings. |
| Context + TUI paint | [plan-perf-context-tui.md](plan-perf-context-tui.md) | done | Token-budget compact, tool result cap, layout height cache, dirty-draw, stream coalesce. |
| Semantic memory | [plan-memory.md](plan-memory.md) | **shipped v1+v2** | Retain, sync, code RAG, subagent banks, optional ONNX. |
| Latency competitors | [plan-latency-competitors.md](plan-latency-competitors.md) | P0+P1 done | Cache, parallel tools, core profile, routing, doom-loop; P2 optional. |
| FEATURES accuracy | [plan-features-improvements.md](plan-features-improvements.md) | done | Matrix fixed; `/tools` `/info` surface latency knobs. |

## Shipped (archived)

| Phase | Archive | Status |
|---|---|---|
| 1 Shell command risk | [archive/phase-1-command-risk.md](archive/phase-1-command-risk.md) | done |
| 4 CI quality budgets | [archive/phase-4-ci-budgets.md](archive/phase-4-ci-budgets.md) | done |
| 7 Multi-agent coordination | [archive/phase-7-multi-agent.md](archive/phase-7-multi-agent.md) | **shipped lightweight (2026-08-07)** — `swarm` + git worktrees + 3-way merge + file claims / toast |
| 8 TUI rendering | [archive/phase-8-tui.md](archive/phase-8-tui.md) | done |
| 9 Shell OS sandbox | [archive/phase-9-sandbox.md](archive/phase-9-sandbox.md) | done |

Index of archives: [archive/README.md](archive/README.md).

## Deferred (post product launch)

| Item | Status | Notes |
|---|---|---|
| ACP — Agent Client Protocol | deferred | Editor ↔ agent (JSON-RPC). `whycode acp` stub only. Not agent-to-agent. |
| `web` surface | stub | Same band as ACP; not blocking launch. |

## Current focus

Priority for shipping the product (aligned with [FEATURES.md](FEATURES.md) gaps):

1. **First public release (almost done)** — `v0.1.0` cut + smoke OK with token. Remaining: **make repo public**, optional Homebrew binary formula, Windows install.ps1 smoke.
2. **Terminal product polish** — latency stack + mouse TUI + FEATURES matrix accurate (2026-08-05). Next: plugins depth, subagent token fold.
3. **Performance residual** — stats done; optional CI ceilings / subagent tokens remain.
4. **OAuth** stays blocked until owner decisions.
5. **Memory v1+v2** shipped (retain, project scope, code RAG, subagent banks, optional ONNX).

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
