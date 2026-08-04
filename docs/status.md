# Roadmap status

Living tracker for open work and past decisions. Update this file in the same
commit as the work it describes.

Last updated: **2026-08-04** (phase docs cleaned: done → `archive/`, open → `plan-*`)

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
| Distribution & self-update | [plan-distribution.md](plan-distribution.md) | implemented | Installers, `upgrade`, release workflow written. Unticked criteria need a published `v*` tag. |
| OAuth & credential discovery | [plan-oauth.md](plan-oauth.md) | blocked | Owner: client registration + provider terms before any code. |
| Performance residual | [plan-performance.md](plan-performance.md) | mostly done | Harness + benchmarks exist. Open: stats aggregation, subagent tokens, provider reconcile, optional CI ceilings. |
| Semantic memory | [plan-memory.md](plan-memory.md) | not started | Needs model-distribution decision + RSS cost comfort. |

## Shipped (archived)

| Phase | Archive | Status |
|---|---|---|
| 1 Shell command risk | [archive/phase-1-command-risk.md](archive/phase-1-command-risk.md) | done |
| 4 CI quality budgets | [archive/phase-4-ci-budgets.md](archive/phase-4-ci-budgets.md) | done |
| 7 Multi-agent coordination | [archive/phase-7-multi-agent.md](archive/phase-7-multi-agent.md) | dropped |
| 8 TUI rendering | [archive/phase-8-tui.md](archive/phase-8-tui.md) | done |
| 9 Shell OS sandbox | [archive/phase-9-sandbox.md](archive/phase-9-sandbox.md) | done |

Index of archives: [archive/README.md](archive/README.md).

## Deferred (post product launch)

| Item | Status | Notes |
|---|---|---|
| ACP — Agent Client Protocol | deferred | Editor ↔ agent (JSON-RPC). `whycode acp` stub only. Not agent-to-agent. |
| `web` surface | stub | Same band as ACP; not blocking launch. |

## Current focus

Priority for shipping the product (not a full backlog rewrite):

1. **First public release** — tag `v*`, exercise installers / `upgrade` (closes residual distribution criteria).
2. **Product polish on the terminal path** — TUI, tools, providers, docs. No ACP/web until after launch.
3. **Performance residual** only if it blocks release confidence (stats schema is nice-to-have).
4. **OAuth** stays blocked until owner decisions.
5. **Memory** stays not-started until deliberately scheduled.

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
