# Roadmap status

Living tracker for the phased plan in `docs/1.md` … `docs/7.md`. Update this
file in the same commit as the work it describes — a phase is not "done"
because the code merged, it is done when its acceptance criteria in the phase
doc are checked off and verified.

Last updated: 2026-07-31

## Legend

| Status | Meaning |
|---|---|
| `not started` | No work has begun |
| `in progress` | Some tasks done, acceptance criteria not all met |
| `blocked` | Waiting on a decision or external dependency |
| `done` | Every acceptance criterion in the phase doc verified |
| `dropped` | Deliberately abandoned; the doc records why |

## Phases

| # | Phase | Doc | Status | Notes |
|---|---|---|---|---|
| 1 | Shell command risk classification | [1.md](1.md) | not started | Highest priority: today `bash = "allow"` gives zero protection |
| 2 | Distribution and self-update | [2.md](2.md) | not started | `upgrade` is currently a stub that prints instructions |
| 3 | OAuth and credential discovery | [3.md](3.md) | not started | Depends on 2 for a credible install story |
| 4 | CI quality budgets | [4.md](4.md) | not started | Cheapest phase; can run in parallel with any other |
| 5 | Performance measurement | [5.md](5.md) | not started | Prerequisite for any performance claim |
| 6 | Semantic memory | [6.md](6.md) | not started | Largest feature; do not start before 1–5 |
| 7 | Multi-agent coordination | [7.md](7.md) | not started | Optional; revisit after 6 |

## Current focus

Nothing in progress. Phase 1 is the recommended starting point.

## Decision log

Decisions that shaped this plan, so they are not re-litigated later.

| Date | Decision | Rationale |
|---|---|---|
| 2026-07-31 | Stop targeting "OpenCode parity" as the project's goal | Parity is definitionally a following position. It gives a user no reason to choose whycode over the thing it copies. See `docs/comparison.md`. |
| 2026-07-31 | Re-implement borrowed designs rather than vendoring jcode source | jcode is MIT, so copying is permitted with attribution, but its abstractions assume its own config, provider and session types. Porting the design is cheaper than porting the code plus its dependencies. Any file that is a derivative work keeps jcode's copyright notice. |
| 2026-07-31 | Safety before features | whycode runs shell commands from a model with no risk classification. That is a correctness problem, not a feature gap, so it precedes everything user-facing. |

## Verification commands

Every phase's acceptance criteria assume these pass:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
