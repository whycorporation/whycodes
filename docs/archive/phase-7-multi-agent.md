# Phase 7 — Multi-agent coordination

**Status:** partial (2026-08-07) · was dropped 2026-07-31 · **Depends on:** 1, 6 · **Blocks:** nothing

**2026-08-07 update:** Lightweight swarm + worktree isolation shipped:

- `swarm` tool (parallel subagents, max 8, config `[swarm]`)
- Git worktrees under `.whycodes/swarm/<run>/worker-N` (`swarm.worktrees = true`)
- Three-way merge back into main; conflicts toast as `FileConflict`
- In-process `FileClaimRegistry` for same-checkout mode / pre-claims
- Long worker reports get a synthetic `TLDR:` when over 2k chars

Still out: full concurrent-agent TUI panel, automatic decomposition. The
original drop rationale below still applies for over-engineering on tiny
repos; the lightweight path is for wide disjoint work when the primary
agent chooses to fan out.

---

Originally dropped after answering the prerequisite question below. The answer
is recorded rather than the phase deleted, so the reasoning is available if the
situation changes.

## Answer to the prerequisite question

> What task is actually faster with three agents than with one?

**For whycodes today: none that we have.**

Parallel agents pay off when work decomposes into independent pieces that each
need their own context — a wide audit, a mechanical migration across hundreds of
files, separate investigations of unrelated subsystems. whycodes is 26k lines
across 19 crates. A change here touches one or two crates, and the coordination
overhead plus merge conflicts would exceed the gain on every task carried out in
this repository so far, including the phases in this plan.

The `task` tool already covers what is genuinely useful: spawning a scoped
subagent for a read-heavy investigation while the primary agent keeps its
context clean. That is the case parallelism actually helps with here, and it is
already handled.

Two things would change the answer:

- A codebase large enough that a single agent cannot hold the relevant context —
  jcode's 600k lines is plausibly there; 26k is not.
- A workload that is genuinely wide by nature, such as a per-file migration
  across hundreds of files, where the decomposition is obvious and the pieces
  do not interact.

Neither is true now. Building coordination machinery, worktree isolation and a
file-ownership protocol for a benefit nobody can name would be complexity for
its own sake — which the risks section below already identified as the most
likely failure mode of this phase.

The design notes below are left intact for whoever revisits this.

## Problem

whycodes has subagents: the `task` tool spawns `general`, `explore` or `scout`,
each runs to completion and reports back. One at a time, one conversation.

jcode's "swarm" runs several agents on one repository concurrently with
conflict resolution between them. Whether that is genuinely useful or mostly a
demo is not something this comparison established, and that uncertainty is why
this phase is last.

## Prerequisite question

**Before writing any code, answer: what task is actually faster with three
agents than with one?**

Parallel agents help when work decomposes into independent pieces that each
need their own context — a wide audit, a mechanical migration across many
files, independent investigations of separate subsystems. They do not help
with a single coherent change, where coordination overhead exceeds the gain
and merge conflicts eat the rest.

If the honest answer is "we do not have such a task", stop here and mark this
phase `dropped` with that reasoning. That is a better outcome than building
coordination machinery nobody uses.

## Goal

Independent, decomposable work runs across several agents on one repository
without them corrupting each other's changes.

## Scope

In, if the prerequisite question is answered affirmatively:

- Concurrent agents with isolated working state — likely git worktrees, which
  give real filesystem isolation rather than a convention.
- Explicit file-ownership claims so two agents do not edit the same file.
- Structured completion reports with a required short summary for long bodies,
  so the TUI can collapse them instead of dumping transcripts.
- Cancellation that reaches every agent.
- TUI presentation of multiple concurrent agents.

Out:

- Distributed execution across machines.
- Agents negotiating with each other in natural language. Ownership should be
  structural, not conversational.
- Automatic decomposition of arbitrary work. The user or the primary agent
  decides the split.

## Tasks

- [ ] Answer the prerequisite question in writing, in this file
- [ ] Design note on isolation: worktrees versus in-process, with the failure
      modes of each
- [ ] Agent registry: spawn, track, cancel
- [ ] File-ownership claims with conflict detection
- [ ] Completion report format with a `tldr` required above a length threshold
- [ ] TUI: concurrent agent display and per-agent cancel
- [ ] Merge strategy when two agents touch the same file despite claims

## Acceptance criteria

- [ ] A named, real task completes measurably faster with N agents than with 1,
      with the measurement recorded
- [ ] Two agents cannot write the same file concurrently
- [ ] Cancelling the session terminates every agent and leaves no orphan
      process or worktree
- [ ] An agent crashing does not take down the others or corrupt the
      repository
- [ ] Worktrees are cleaned up on both success and failure
- [ ] The TUI remains readable with three agents streaming at once
- [ ] Phase 1's risk classification applies to every agent, not only the
      primary one

## Risks

- **Complexity for its own sake.** This is the phase most likely to be a
  demo rather than a tool. The prerequisite question exists to catch that.
- **Repository corruption.** Concurrent writes to one checkout is the failure
  mode. Worktrees cost setup time but make isolation real.
- **Permission bypass.** Every agent must go through the same gate. A subagent
  that skips Phase 1's classification undoes Phase 1.
- **Unreadable output.** Three streams into one transcript is noise. The
  `tldr` requirement is the mitigation.

## Reference

`jcode/crates/jcode-swarm-core` (767 lines) — see `validate_swarm_tldr` and the
`MAX_SWARM_COMPLETION_REPORT_CHARS` / `SWARM_TLDR_REQUIRED_OVER_CHARS`
constants for how they bound report size. `jcode/scripts/test_swarm.py` and
`benchmark_swarm.py`.
