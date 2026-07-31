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
| 1 | Shell command risk classification | [1.md](1.md) | done | `crates/command-risk`, gated in `Agent::execute_with_permission` |
| 2 | Distribution and self-update | [2.md](2.md) | not started | `upgrade` is currently a stub that prints instructions |
| 3 | OAuth and credential discovery | [3.md](3.md) | not started | Depends on 2 for a credible install story |
| 4 | CI quality budgets | [4.md](4.md) | not started | Cheapest phase; can run in parallel with any other |
| 5 | Performance measurement | [5.md](5.md) | not started | Prerequisite for any performance claim |
| 6 | Semantic memory | [6.md](6.md) | not started | Largest feature; do not start before 1–5 |
| 7 | Multi-agent coordination | [7.md](7.md) | not started | Optional; revisit after 6 |
| 8 | TUI rendering and readability | [8.md](8.md) | in progress | Markdown, highlighting and JSON themes done; pickers and toasts remain |

## Current focus

Phase 1 is complete. Phase 2 (distribution) or Phase 4 (budgets, independent of
everything else) are the next candidates.

## Decision log

Decisions that shaped this plan, so they are not re-litigated later.

| Date | Decision | Rationale |
|---|---|---|
| 2026-07-31 | Stop targeting "OpenCode parity" as the project's goal | Parity is definitionally a following position. It gives a user no reason to choose whycode over the thing it copies. See `docs/comparison.md`. |
| 2026-07-31 | Re-implement borrowed designs rather than vendoring jcode source | jcode is MIT, so copying is permitted with attribution, but its abstractions assume its own config, provider and session types. Porting the design is cheaper than porting the code plus its dependencies. Any file that is a derivative work keeps jcode's copyright notice. |
| 2026-07-31 | Safety before features | whycode runs shell commands from a model with no risk classification. That is a correctness problem, not a feature gap, so it precedes everything user-facing. |
| 2026-07-31 | Default `bash_risk_threshold` is `destructive`, not `caution` as 1.md first proposed | `caution` fires on ordinary in-project cleanup (`rm -rf target`, `> file`). A gate that prompts during a normal build gets switched off, and then protects nothing. |
| 2026-07-31 | Unresolvable targets escalate to `destructive`, never `catastrophic` | `catastrophic` is not promptable. An unexpandable `$BUILD_DIR` or a `$(…)` target is unknown, not known-bad, so refusing it outright would block legitimate work with no way to override. Refusal is reserved for targets we positively identified. |
| 2026-07-31 | Unrecognised commands are `safe` | The alternative — unknown means dangerous — prompts on every build and script. Recorded as a limitation in the crate docs and README rather than hidden. |
| 2026-07-31 | Added phase 8 (TUI), targeting opencode's look and feel | The original seven phases had no TUI phase, which was a gap: the TUI is the product surface. opencode's TUI is SolidJS on OpenTUI, so no code transfers — but its theme JSON schema does, and 33 themes come with it. |

## Verification commands

Every phase's acceptance criteria assume these pass:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
