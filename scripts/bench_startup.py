#!/usr/bin/env python3
"""Measure how long whycode takes to start.

Runs the binary N times and reports the median and p95 wall time. The first run
is discarded: it pays for reading the binary off disk, and every run after it
does not, so including it measures the filesystem cache rather than the program.

What this measures is process start plus argument parsing plus whatever the
subcommand does — not time to first rendered TUI frame, which needs a terminal
and is measured separately. `--version` is therefore the floor: no program can
start faster than this, and any TUI number sits on top of it.

Usage:
    python scripts/bench_startup.py [--runs 20] [--binary path] [--json out.json]
"""

from __future__ import annotations

import argparse
import json
import platform
import statistics
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Each case is (label, args). Ordered cheapest first so a regression in the
# floor is visible separately from a regression in a subcommand.
CASES = [
    ("version", ["--version"]),
    ("help", ["--help"]),
    ("config-show", ["config", "show"]),
]


def default_binary() -> Path:
    name = "whycode.exe" if platform.system() == "Windows" else "whycode"
    for profile in ("release", "debug"):
        candidate = ROOT / "target" / profile / name
        if candidate.exists():
            return candidate
    raise SystemExit(
        "no whycode binary found — build one first:\n  cargo build --release -p whycode-cli"
    )


def time_run(binary: Path, args: list[str]) -> float:
    """Wall milliseconds for one invocation."""
    start = time.perf_counter()
    subprocess.run(
        [str(binary), *args],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    return (time.perf_counter() - start) * 1000.0


def measure(binary: Path, args: list[str], runs: int) -> dict[str, float]:
    # Discard the first run rather than averaging it in.
    time_run(binary, args)
    samples = sorted(time_run(binary, args) for _ in range(runs))
    return {
        "runs": runs,
        "median_ms": round(statistics.median(samples), 2),
        "p95_ms": round(samples[min(int(len(samples) * 0.95), len(samples) - 1)], 2),
        "min_ms": round(samples[0], 2),
        "max_ms": round(samples[-1], 2),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--runs", type=int, default=20)
    parser.add_argument("--binary", type=Path, default=None)
    parser.add_argument("--json", type=Path, default=None)
    args = parser.parse_args()

    binary = args.binary or default_binary()
    profile = "release" if "release" in str(binary) else "debug"

    results = {
        "binary": str(binary.relative_to(ROOT)) if binary.is_relative_to(ROOT) else str(binary),
        "profile": profile,
        "platform": f"{platform.system()} {platform.machine()}",
        "cases": {},
    }

    print(f"{binary}  ({profile}, {results['platform']})")
    print(f"{'case':<14} {'median':>9} {'p95':>9} {'min':>9} {'max':>9}")
    for label, argv in CASES:
        stats = measure(binary, argv, args.runs)
        results["cases"][label] = stats
        print(
            f"{label:<14} {stats['median_ms']:>8.1f}ms {stats['p95_ms']:>8.1f}ms "
            f"{stats['min_ms']:>8.1f}ms {stats['max_ms']:>8.1f}ms"
        )

    if profile != "release":
        print("\nnote: this is a debug build; release numbers are the ones worth quoting")

    if args.json:
        args.json.write_text(json.dumps(results, indent=2) + "\n", encoding="utf-8")
        print(f"\nwrote {args.json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
