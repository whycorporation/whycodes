# Roadmap status

Living tracker for the phased plan in `docs/1.md` … `docs/7.md`. Update this
file in the same commit as the work it describes — a phase is not "done"
because the code merged, it is done when its acceptance criteria in the phase
doc are checked off and verified.

Last updated: 2026-08-04 (phases 1, 4, 8 done; 2 implemented; 5 partial; 7 dropped; shell OS sandbox shipped)

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
| 2 | Distribution and self-update | [2.md](2.md) | implemented | Release workflow, installers and real self-update; criteria needing a published release are unticked in 2.md |
| 3 | OAuth and credential discovery | [3.md](3.md) | blocked | Needs a provider-terms review and client registration — an owner decision, see 3.md |
| 4 | CI quality budgets | [4.md](4.md) | done | panic, swallowed-error and dependency-edge ratchets; binary size deferred to phase 2 |
| 5 | Performance measurement | [5.md](5.md) | mostly done | Startup, RSS, first frame, idle draws and token accounting all measured; stats aggregation and the CI gate remain |
| 6 | Semantic memory | [6.md](6.md) | not started | Largest phase; open model-distribution decision, see 6.md |
| 7 | Multi-agent coordination | [7.md](7.md) | dropped | Prerequisite question answered in 7.md: no task here is faster with three agents |
| 8 | TUI rendering and readability | [8.md](8.md) | done | Markdown, highlighting, JSON themes, pickers and toasts; render-cost criterion waits on phase 5 |

## Current focus

Nothing in progress. Three things are ready to pick up, in order of what is
actually blocking:

1. **Phase 5's pty harness** — time to first frame and idle draws. Blocks
   Phase 6, and is the only measurement that would be comparable to what other
   agents publish.
2. **Phase 2's first release** — one `git tag`, and the seven unticked criteria
   in 2.md become testable.
3. **Phase 3's two owner decisions** — OAuth client registration and a
   per-provider terms reading. Neither is an engineering task.

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
| 2026-08-04 | Shell OS sandbox defaults to `workspace` with network on and fallback allow | Risk parse is not a sandbox; bwrap workspace is the second lock. Network stays on so cargo/npm/git work; fallback allow avoids breaking macOS/Windows/CI without bwrap. |
| 2026-08-04 | **ACP (Agent Client Protocol) deferred until after product launch** | Ship the terminal product first. `whycode acp` stays a stub. Real ACP (editor ↔ agent, JSON-RPC/stdio, per [agentclientprotocol.com](https://agentclientprotocol.com)) is post-release work for IDE surfaces — not agent-to-agent. Do not start ACP implementation before a first public product release. |

## Deferred (post-release)

| Item | Status | Notes |
|---|---|---|
| ACP — Agent Client Protocol | deferred | Owner: after product ships. Official name is **Agent Client Protocol** (not “Control”). Goal: whycode as ACP agent for Zed / other ACP clients. Stub only until then. |
| `web` surface | stub | Same priority band as ACP; not blocking launch. |

## Verification commands

Every phase's acceptance criteria assume these pass:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```










