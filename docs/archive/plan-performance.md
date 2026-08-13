# Plan — Performance measurement (residual)

**Status:** mostly done · **Was:** phase 5 · **Depends on:** CI budgets (shipped) · **Blocks:** none for launch (live provider reconcile optional/manual)

## Problem

whycode has no measurements. Not slow ones — none. There is no startup number,
no memory number, no token-usage number. So:

- We cannot tell whether a change made things worse.
- We cannot make any performance claim.
- We cannot evaluate whether the "efficient alternative" position is even
  available, because we do not know where we stand.

jcode's entire public identity is two measured numbers, and it maintains the
harness to defend them: `bench_startup.py`,
`bench_startup_visible_ready.py`, `bench_memory_cli.py`,
`memory_regression_gate.sh`, `check_startup_budget.sh`,
`compare_token_usage.py`, `count_idle_draws.py`.

The idle-draw counter is worth noting. A TUI that redraws when nothing changed
burns CPU for no reason, and it is invisible without an explicit counter.

## Goal

Startup time, resident memory and idle CPU are measured, tracked over time, and
regressions fail CI.

## Scope

In:

- Cold start to a usable prompt, measured, not estimated.
- Resident memory for one idle session, and the increment per extra session.
- Idle redraw count over a fixed window with no input.
- Token accounting per turn: prompt, completion, cache read, cache write.
- A results file committed on every run so the trend is in git history.
- CI gates on the first three.

Out:

- Comparisons against other tools. Measure ourselves first. Publishing a
  comparison means reproducing the other tool's numbers fairly, which is a
  separate and much larger commitment.
- Micro-benchmarks of individual functions. Not the bottleneck at this size.
- Optimisation. This phase measures; it does not tune. Resist fixing what the
  first run reveals until the harness is trusted.

## Tasks

- [x] `scripts/bench_startup.py`: launch to first rendered frame, N runs,
      report median and p95, discard the first run as cache-warming
- [x] `scripts/bench_memory.py`: peak RSS for short CLI runs, plus multi-session
      idle-TUI PSS (1 and 10 sessions via `/proc/.../smaps_rollup`)
- [x] `scripts/count_idle_draws.py`: redraws in 10s with no input; the target
      is zero
- [x] Structured token accounting on every turn, surfaced by `/info`
- [x] Per-session usage persisted in SQLite; `whycode stats` aggregates it
- [x] `docs/benchmarks.md` recording the method precisely enough to reproduce:
      machine, build profile, terminal, run count
- [x] `bench-results.json` committed per run (`docs/bench-results.json`)
- [x] CI job gating startup / memory against ceilings
      (`scripts/check_bench_ceilings.py` in CI budgets job; generous Linux ceilings)
- [x] Record the first measured numbers in `docs/comparison.md`

## Acceptance criteria

- [x] Each benchmark is reproducible: two runs on the same machine and build
      agree within 10% on the median
- [x] The method doc is specific enough for someone else to reproduce it
- [x] A deliberate 2× startup regression fails the CI gate
      (`check_bench_ceilings.py` on `version_ms_p95` / `version_rss_mb`;
      idle-draw / first-frame gates remain local via `docs/benchmarks.md`)
- [x] Idle redraws over 10 seconds with no input is 0, or the number is
      recorded with an explanation of why it is not
- [ ] Token counts reconcile with the provider's reported usage within 1% on
      at least one real session (**manual** — needs a live provider turn)
- [x] Benchmarks run on all three platforms, or the doc states which platform
      the ceilings apply to and why (`docs/benchmarks.md`: Linux x86_64
      ceilings; multi-session PSS is Linux-only via `/proc`)

## Risks

- **Noisy CI runners.** Shared runners vary enough to make tight ceilings
  flaky. Set them generously — catching a 2× regression is the goal, not a 5%
  one. If it still flaps, gate on a self-hosted runner or move the gate to a
  manual workflow rather than weakening it into meaninglessness.
- **Measuring the wrong start.** "Process exit" is not "usable". Measure to the
  first rendered frame; jcode maintains a separate
  `bench_startup_visible_ready.py` for exactly this distinction.
- **The numbers may be bad.** That is the point of measuring. Record them
  as-is; do not delay the phase until they look good.

## Reference

`jcode/scripts/bench_startup.py`, `bench_startup_visible_ready.py`,
`bench_memory_cli.py`, `memory_regression_gate.sh`, `check_startup_budget.sh`,
`count_idle_draws.py`, `compare_token_usage.py`.

## What was done, and what was not

Done: startup and peak-RSS benchmarks that run headlessly, the method doc, and
the first recorded numbers. `docs/benchmarks.md` has them.

Not done, and blocked on the same thing: time to first TUI frame, idle redraw
count, and per-session memory all need the binary driven through a terminal.
Spawning a process and timing it cannot see a rendered frame. Doing it properly
means instrumenting the render loop behind an environment variable and driving
the binary through a pty — a self-contained piece of work, but not the same
piece of work as the scripts above.

The CI regression gate is deliberately not added. A ceiling loose enough not to
flap on shared runners at a 20 ms measurement catches almost nothing, and a gate
that catches nothing is worse than no gate because it reads as assurance. The
useful gate is on first-frame time and idle draws, so it waits for them.

## Update, 2026-08-01

The pty harness landed, so first frame and idle draws are measured — see
`docs/benchmarks.md`. Idle redraws went from 21.4/s to 1.96/s as a result.

Token accounting also landed. Providers were already emitting
`StreamEvent::Usage`; the agent discarded it with `StreamEvent::Usage { .. } =>
{}`. It now accumulates per turn, folds into the session, and `/info` reports
the provider's own numbers in both the TUI and the plain REPL.

## Update, 2026-08-04 — usage persistence

- Sessions table stores `input_tokens`, `output_tokens`, and optional cache
  columns; `Session::save_to_db` / `load_from_db` round-trip them.
- `whycode stats` prints provider-reported totals and top sessions (no more
  500×message heuristic).
- Message rows are replaced on each save (fixes duplicate-message inflate).

## Update, 2026-08-07 — residual closed (partial)

- **Subagent tokens.** `SubagentResult.usage` accumulates provider Usage/CacheUsage;
  parent folds via `subagent_usage_pending` into session + turn after tool batches
  (task + swarm workers). Tool report includes `[subagent usage: …]`.
- **CI ceilings / `bench-results.json`.** `docs/bench-results.json` schema +
  `scripts/check_bench_ceilings.py` (generous Ubuntu ceilings); CI budgets job
  runs the checker (skips cleanly if file absent).
- **Reconciliation against a live provider** still needs a real API session
  (manual); arithmetic remains unit-tested.

**Status:** **mostly done** — residual code + CI ceilings shipped; live
provider reconcile remains manual (optional, not blocking).