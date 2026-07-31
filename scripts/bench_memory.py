#!/usr/bin/env python3
"""Measure peak resident memory for a whycode invocation.

Peak RSS is what a user perceives as "how much memory does this thing take", and
it is the figure other terminal agents advertise. Measured here per subcommand
so a regression can be attributed rather than just observed.

Requires `psutil`. Without it the script says so and exits 0 rather than
failing: this is a reporting tool, and a missing optional dependency should not
break a pipeline that also runs the budgets.

Usage:
    python scripts/bench_memory.py [--runs 5] [--binary path] [--json out.json]
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

CASES = [
    ("version", ["--version"]),
    ("config-show", ["config", "show"]),
    ("session-list", ["session", "list"]),
]

# How often to sample RSS. Short-lived processes need a tight loop or the peak
# is missed entirely; this is the trade-off between accuracy and burning CPU.
SAMPLE_INTERVAL_S = 0.001


def default_binary() -> Path:
    name = "whycode.exe" if platform.system() == "Windows" else "whycode"
    for profile in ("release", "debug"):
        candidate = ROOT / "target" / profile / name
        if candidate.exists():
            return candidate
    raise SystemExit(
        "no whycode binary found — build one first:\n  cargo build --release -p whycode-cli"
    )


def peak_rss_mb(psutil, binary: Path, args: list[str]) -> float:
    """Peak RSS in MiB for one invocation, sampled while it runs."""
    process = subprocess.Popen(
        [str(binary), *args],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    handle = psutil.Process(process.pid)
    peak = 0

    while process.poll() is None:
        try:
            peak = max(peak, handle.memory_info().rss)
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            break
        time.sleep(SAMPLE_INTERVAL_S)

    process.wait()
    return round(peak / (1024 * 1024), 2)


def main() -> int:
    try:
        import psutil  # noqa: PLC0415
    except ImportError:
        print("psutil is not installed; skipping memory benchmark")
        print("  pip install psutil")
        return 0

    parser = argparse.ArgumentParser()
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--binary", type=Path, default=None)
    parser.add_argument("--json", type=Path, default=None)
    args = parser.parse_args()

    binary = args.binary or default_binary()
    profile = "release" if "release" in str(binary) else "debug"

    results = {
        "binary": str(binary),
        "profile": profile,
        "platform": f"{platform.system()} {platform.machine()}",
        "cases": {},
    }

    print(f"{binary}  ({profile}, {results['platform']})")
    print(f"{'case':<14} {'median':>10} {'max':>10}")
    for label, argv in CASES:
        samples = sorted(peak_rss_mb(psutil, binary, argv) for _ in range(args.runs))
        stats = {
            "runs": args.runs,
            "median_mb": round(statistics.median(samples), 2),
            "max_mb": samples[-1],
        }
        results["cases"][label] = stats
        print(f"{label:<14} {stats['median_mb']:>9.1f}M {stats['max_mb']:>9.1f}M")

    if args.json:
        args.json.write_text(json.dumps(results, indent=2) + "\n", encoding="utf-8")
        print(f"\nwrote {args.json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
