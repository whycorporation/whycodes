# Benchmarks

Phase 5 of [5.md](5.md). Before this, whycode had no measurements at all — not
slow ones, none — so there was no way to tell whether a change made things worse
and no basis for any performance statement.

## Running them

```bash
cargo build --release -p whycode-cli
python scripts/bench_startup.py --runs 20
python scripts/bench_memory.py --runs 5      # needs psutil
```

Both accept `--json out.json` so a run can be recorded.

## Method

**Startup** (`bench_startup.py`) times the binary end to end, N times, and
reports the median and p95. The first run is discarded: it pays for reading the
binary off disk and every run after it does not, so including it measures the
filesystem cache rather than the program.

**Memory** (`bench_memory.py`) samples RSS every millisecond while the process
runs and takes the peak. A tight loop is necessary because these processes live
for tens of milliseconds; a slower sample misses the peak entirely.

Both refuse to guess: they require an existing binary and say which profile it
came from, because a debug number is not worth quoting.

## First measurements

Taken 2026-07-31 on Windows 11, AMD64, release profile, whycode 0.1.0.

| Case | Startup median | Startup p95 | Peak RSS |
|---|---|---|---|
| `--version` | 20.9 ms | 22.6 ms | 9.6 MB |
| `--help` | 21.4 ms | 27.0 ms | — |
| `config show` | 25.6 ms | 30.3 ms | 11.7 MB |
| `session list` | — | — | 12.3 MB |

`--version` is the floor: process start, binary load and argument parsing, with
no work on top. Everything else sits above it, and the ~5 ms between `--version`
and `config show` is config loading and the SQLite open.

These are single-machine numbers on a developer laptop, not a controlled
environment. They are useful as a baseline to compare future runs against, not
as a published claim.

## What is deliberately not measured yet

Being explicit, because a benchmark page that quietly omits things reads as
though it covered them.

- **Time to first TUI frame.** The number that matters most for perceived speed,
  and the one other agents advertise. It needs a terminal, so it cannot be
  measured by spawning a process and timing it. Measuring it properly means
  instrumenting the render loop behind an environment variable and driving the
  binary through a pty.
- **Idle redraws.** A TUI that redraws when nothing changed burns CPU for no
  reason, and it is invisible without an explicit counter. Same blocker.
- **Memory per additional session.** Requires driving a real session, which
  requires a provider key.
- **Token accounting.** Listed in 5.md; not implemented.

## Comparison with other tools

Not attempted, on purpose. Publishing a comparison means reproducing the other
tool's numbers fairly — same machine, same terminal, same definition of
"started" — which is a separate and much larger commitment than measuring
ourselves. jcode's published figures (27.8 MB, 14.0 ms time-to-first-frame) are
its own claims and remain unverified here; note also that they are not measuring
the same thing as the table above.

## Regression gating

Not wired into CI yet. Shared runners vary enough that a tight ceiling would
flap, and a ceiling loose enough not to flap on a 20 ms measurement catches
almost nothing. The useful gate is on time to first frame and idle draws, which
are exactly the two things not yet measurable — so the gate waits for them
rather than being added in a weakened form that provides false assurance.
