# Benchmarks

Phase 5 of [5.md](5.md). Before this, whycode had no measurements at all — not
slow ones, none — so there was no way to tell whether a change made things worse
and no basis for any performance statement.

## Running them

Process level:

```bash
cargo build --release -p whycode-cli
python scripts/bench_startup.py --runs 20
python scripts/bench_memory.py --runs 5      # needs psutil
```

Both accept `--json out.json` so a run can be recorded.

Function level, for the two paths that run on a hot loop:

```bash
cargo bench -p whycode-format        # markdown + highlighting, per frame
cargo bench -p whycode-command-risk  # shell risk gate, per tool call
```

These use criterion. They are not run in CI — criterion's own comparison against
the previous run is per machine, and a shared runner would report noise as a
regression. They are for changing the code in these paths and checking the
before and after on one machine.

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

## Hot paths

Added 2026-07-31 after the process-level numbers, for the two functions that do
not run once per invocation.

`highlight_code_spans` and `parse_markdown` are called from the TUI render loop,
so their cost is paid **per frame, per visible message** — not once per
response. `assess` runs before every `bash` tool call.

Both benchmarks found a defect on their first run, which is the argument for
having them at all.

| | before | after | |
|---|---|---|---|
| `assess` on `cargo test --workspace --no-fail-fast` | 39.8 µs | 2.11 µs | 18.8× |
| `assess` on `ls -la` | 6.12 µs | 780 ns | 7.8× |
| `highlight_code_spans`, 100 lines of Rust | 5.83 ms | 92 µs | 63× |
| `highlight_code_spans`, 500 lines | 29.3 ms | 468 µs | 63× |

**The tokeniser allocated per character.** `match_operator` collected each of
its fifteen candidate operators into a `Vec<char>` at every character position,
so cost scaled with input length times candidate count. The measured ratio
between the two commands matched their character-count ratio exactly, which is
what pointed at it. Fixed by comparing char by char.

**Highlighting ran every frame.** 5.8 ms for a 100-line Rust block, against
17 µs for the same text untagged — syntect is roughly 350× the cost of not
highlighting, and the TUI was paying it on every redraw. A visible 500-line
block would have cost 29 ms per frame. Fixed with a bounded memo on
`(code, language)`; the block is identical between frames, so it is computed
once.

The ANSI path (`render_markdown`, used by `--plain` and the CLI) still
highlights uncached at ~3.8 ms for a typical response. That one runs once per
response rather than once per frame, so it is left alone.

## Time to first frame, and idle redraws

Added 2026-07-31. These are the two numbers a process-level benchmark cannot
reach: timing a process that has exited says nothing about when it drew, and a
loop that repaints an unchanged screen is invisible without a counter.

```bash
python scripts/bench_first_frame.py --runs 12 --idle-ms 0      # first frame
python scripts/bench_first_frame.py --runs 10 --idle-ms 3000   # idle redraws
```

The binary reports on itself when `WHYCODE_BENCH` is set — see
`crates/tui/src/bench.rs`. It is inert otherwise: two atomic loads per frame.
The harness allocates a pty on Unix; Windows has no stdlib ConPTY, so the child
inherits the console and the screen flickers briefly during a run.

| | median | min | max |
|---|---|---|---|
| First frame, from the first statement of `main` | **4.74 ms** | 4.29 ms | 4.98 ms |
| Spawn to exit, `--idle-ms 0` | 27.4 ms | 26.5 ms | 28.8 ms |
| Idle redraws per second | **1.96** | 1.95 | 1.97 |

The two timings decompose the wait. Inside the process, config loading through
to a painted screen is 4.7 ms. Externally the same run takes 27.4 ms, so
roughly 22 ms is process creation, dynamic linking and teardown — cost a user
pays that the process cannot see. That figure cross-checks against `--version`
at 23.8 ms, which does nothing but start, parse and exit.

**Idle redraws were 21.4 per second.** The loop draws once per iteration and
polled for input with a fixed 40 ms timeout, so with nobody typing it repainted
an unchanged screen twenty-one times a second. The timeout is now 500 ms when
nothing is live, and stays at 40 ms while the agent is streaming or a toast is
counting down — the things that do not arrive as terminal events and so need
the loop to come round. Input latency is unaffected either way, because `poll`
returns the moment a key arrives.

That took idle redraws from 21.44/s to 1.96/s, a 91% reduction, with first-frame
time unchanged at 4.74 ms.

Zero is still the right target and this is not zero. Reaching it means only
drawing when something changed, which means tracking invalidation across every
state mutation — and a missed one leaves a stale screen, which is a worse
failure than a wasted repaint. The timeout change gets most of the benefit
without that risk.

## What is deliberately not measured yet

Being explicit, because a benchmark page that quietly omits things reads as
though it covered them.

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

Not wired into CI. First frame and idle draws are now measurable, which removes
the original blocker, but they need a terminal — and CI has none. The pty path
would work on the Linux runner; the Windows and macOS jobs would not.

A gate that runs on one of three platforms is still worth having for a
regression as large as the 21-to-2 change above, and that is the shape to build
if this is wired up: Linux only, on idle draws and first frame, with a ceiling
loose enough to survive a shared runner. Startup and RSS stay ungated — a
ceiling loose enough not to flap at a 20 ms measurement catches almost nothing,
and a gate that catches nothing reads as assurance while providing none.
