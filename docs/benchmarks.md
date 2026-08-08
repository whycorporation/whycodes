# Benchmarks

Living method doc for performance measurement (open residual work:
[plan-performance.md](plan-performance.md)). Before this, whycode had no
measurements at all — not slow ones, none — so there was no way to tell whether
a change made things worse and no basis for any performance statement.

## Running them

Process level:

```bash
cargo build --release -p whycode-cli
python scripts/bench_startup.py --runs 20
python scripts/bench_memory.py --runs 5                  # peak RSS + multi-session PSS
python scripts/bench_memory.py --skip-cli --sessions 1 10  # just the 1 / 10 session PSS table
```

Peak RSS needs `psutil`. Multi-session PSS needs Linux (`/proc/.../smaps_rollup`)
and does not need psutil. Both accept `--json out.json` so a run can be recorded.

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

**Memory** (`bench_memory.py`) has two modes:

1. **Peak RSS** of short CLI subcommands. Samples RSS every millisecond while
   the process runs and takes the peak. A tight loop is necessary because these
   processes live for tens of milliseconds; a slower sample misses the peak.
2. **Multi-session PSS** of concurrent idle TUI sessions. Spawns N
   `whycode run` processes under ptys, answers terminal capability probes so
   the first frame actually paints, settles, then sums Proportional Set Size
   from `/proc/<pid>/smaps_rollup` across each process tree. PSS is what the
   comparison table uses for "10 session" figures (shared pages are not
   counted N times). Default counts are 1 and 10; override with
   `--sessions 1 5 10`. Linux only.

Both refuse to guess: they require an existing binary and say which profile it
came from, because a debug number is not worth quoting.

## First measurements

Taken 2026-07-31 on Windows 11, AMD64, release profile, whycode 0.1.0
(pre-optimisation baseline).

| Case | Startup median | Startup p95 | Peak RSS |
|---|---|---|---|
| `--version` | 20.9 ms | 22.6 ms | 9.6 MB |
| `--help` | 21.4 ms | 27.0 ms | — |
| `config show` | 25.6 ms | 30.3 ms | 11.7 MB |
| `session list` | — | — | 12.3 MB |

### Re-measure, 2026-08-05 (Linux x86_64, release)

**Pass 1** (Tokio deferral + `panic=abort`): early `--version`/`-V`, parse
before runtime, current-thread for light cmds → ~1.3 ms / 14 MB.

**Pass 2** (size + linker): full LTO, partial RELRO (no `BIND_NOW`), drop
tiktoken BPE tables, mermaid + two-face off by default (`--features full` to
restore), optional `server` / `self-update` features.

| Case | Startup median | Startup p95 | notes |
|---|---|---|---|
| `--version` | **1.0 ms** | 1.2 ms | was ~2.1 ms → 1.3 ms → **1.0 ms** |
| `--help` | **1.5 ms** | 2.1 ms | |
| `config show` | **2.1 ms** | 2.6 ms | current-thread runtime |
| binary size | **12 MB** | — | was 16 MB → 14 MB → **12 MB** |

`--version` is still the floor: process start + enough of the binary to print a
string. Everything else sits above it. The Windows ~21 ms figure above was
dominated by paging a larger binary; re-run `bench_startup.py` there after a
release rebuild to refresh that row.

Slim extras: `cargo build --release -p whycode-cli --features full` re-enables
Unicode mermaid + bat/two-face languages.

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
| `highlight_code_spans`, 100 lines of Rust (2026-07-31) | 5.83 ms | 92 µs | 63× |
| `highlight_code_spans`, 500 lines (2026-07-31) | 29.3 ms | 468 µs | 63× |

### Re-measure, 2026-08-04 (Linux, release/`--quick`)

Criterion on this machine after FxHash + **Arc closed memo** (cache hit no
longer deep-clones every span). Cold = first paint of a new body; warm =
closed-memo hit (idle/scroll frames for a finished fence).

**`highlight_code_spans`**

| Case | cold | warm |
|---|---|---|
| Rust, 10 lines | 1.54 ms | **55 ns** |
| Rust, 100 lines | 13.2 ms | **261 ns** |
| Rust, 500 lines | 51.3 ms | **1.26 µs** |
| untagged, 100 lines (warm) | — | 213 ns |

Compared with the 2026-07-31 “after memo” warm path (~84–92 µs for 100 lines),
warm is ~**300×** cheaper: the remaining cost was cloning `Vec<Vec<CodeSpan>>`
every frame, not re-highlighting. Arc clone + FxHash of the source is enough.

Cold is higher than the old combined figure because the old bench hit the memo
on every sample after the first; cold here forces a unique body each iteration
so it measures syntect only.

**`parse_markdown`**

| Case | time |
|---|---|
| typical response | 4.67 µs |
| streaming prefix 200 / 1000 / 4000 chars | 2.55 / 10.9 / 45.0 µs |

**`assess` (command-risk)**

| Case | time |
|---|---|
| safe short (`ls -la` class) | 496 ns |
| safe build (`cargo test …`) | 1.84 µs |
| caution / destructive / catastrophic | 2.15 / 2.27 / 1.23 µs |
| pipeline | 4.40 µs |

**`render_markdown` (ANSI, uncached per call)**

| Case | time |
|---|---|
| typical response | 1.29 ms |
| 200-line Rust fence | 41.5 ms |

Still paid once per response on the CLI/`--plain` path, not per TUI frame.

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

### Update, 2026-08-04 — dirty-draw + context economy

See [archive/plan-perf-context-tui.md](archive/plan-perf-context-tui.md). After this work the
event loop only paints when `needs_redraw` is set, the agent is busy (spinner),
or a toast is live. Idle sessions with no input should report **~0 draws/s**
after the first frame (re-measure with
`python scripts/bench_first_frame.py --runs 10 --idle-ms 3000`). The previous
~2/s figure came from repainting on every 500 ms poll timeout.

Also landed in the same pass:

- **Token-budget compact** — drops oldest messages until under `¾ · max_tokens`
  (heuristic chars/4), keeps ≥4 tail messages, caps tool bodies at 32 768 chars.
- **Layout height cache** — scroll/selection reuses per-message row counts
  instead of full `render_message` for every bubble.
- **Stream delta coalesce** — consecutive `TextDelta` / `ThinkingDelta` events
  in one channel drain become a single append.

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

## Multi-session PSS

Taken 2026-08-04 on Linux x86_64, release profile. Idle TUI sessions (no
provider key, no user input), 2 s settle after spawn, 3 runs, median:

| Sessions | Median PSS | Notes |
|---|---|---|
| 1 | **4.1 MB** | one idle TUI |
| 10 | **16.8 MB** | ten concurrent idle TUIs |
| per added session | **~1.4 MB** | (10 − 1) / 9 |

Method matches the shape jcode publishes: N live interactive processes, PSS
from `smaps_rollup`, not RSS. These are idle sessions (prompt drawn, no
agent turn), so they are a lower bound on a working multi-session desk — a
turn with tool output will cost more private memory per session.

Reproduce:

```bash
cargo build --release -p whycode-cli
python scripts/bench_memory.py --skip-cli --sessions 1 10 --runs 3 --settle 2
```

## What is deliberately not measured yet

Being explicit, because a benchmark page that quietly omits things reads as
though it covered them.

- **Memory of a busy session.** Idle TUI only; a turn with tool output and a
  long transcript is not exercised (would need a provider key or a fixture).
- **Token accounting reconciliation.** Providers report usage; end-to-end
  check against a real session is still open.

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

## Binary size, hashing, and token heuristics (2026-08-04)

See also [archive/plan-perf-hotpath.md](archive/plan-perf-hotpath.md).

**Release profile.** Workspace `[profile.release]` now sets `strip = true`,
`lto = "thin"`, and `codegen-units = 1`. On Linux x86_64 (2026-08-04):

| Binary | Size | Notes |
|--------|------|--------|
| Before (no profile) | **23 MB** | unstripped |
| Manual `strip` only | **18 MB** | same build, symbols dropped |
| After profile + thin LTO | **15 MB** | already stripped |

Roughly **~35% smaller** than the old unstripped release, **~17%** smaller than
strip-only.

**Hashing.** Two different jobs, two different hashes:

| Use | Hash | Why |
|-----|------|-----|
| `whycode upgrade` integrity (`SHA256SUMS`) | **SHA-256** (`sha2`) | Cryptographic; do not replace |
| Highlight / mermaid memo keys; tool & provider registries | **FxHash** (`rustc-hash`) | Trusted local keys; SipHash was paying DoS resistance every TUI frame |

Memo keys are hashed on every render of a visible code/mermaid block even when
the cache hits, so the hasher cost is on the frame budget.

**Math / token heuristics.** `chars_to_tokens_fallback` uses
`chars.div_ceil(4).max(1)` so short Unicode strings are not under-counted as
zero. Tiktoken uses the crate's process-wide BPE singletons
(`cl100k_base_singleton` / `o200k_base_singleton`) so vocab load is paid once.
`token_counter` is exported from `whycode-llm` (it was previously orphaned).

