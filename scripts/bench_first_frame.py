#!/usr/bin/env python3
"""Measure time to first rendered frame, and idle redraws.

These are the two numbers a process-level benchmark cannot reach. Timing a
process that has exited says nothing about when it drew, and a loop that
repaints when nothing changed is invisible without a counter.

The binary reports on itself when `WHYCODE_BENCH` is set (see
`crates/tui/src/bench.rs`); this script drives it and reads the file. Two
timings come out of that:

    first_frame_ms   from inside the process, measured from the first
                     statement of main
    spawn_to_exit_ms measured here, so it includes process creation and
                     dynamic linking — cost a user pays that the process
                     cannot see

The TUI needs a real terminal. On Unix this allocates a pty. On Windows there
is no stdlib pty, so the child inherits this console: run it from a terminal,
and expect a brief flicker as the alternate screen is entered and left.

Usage:
    python scripts/bench_first_frame.py [--runs 10] [--idle-ms 2000]
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
IS_WINDOWS = platform.system() == "Windows"


def default_binary() -> Path:
    name = "whycode.exe" if IS_WINDOWS else "whycode"
    for profile in ("release", "debug"):
        candidate = ROOT / "target" / profile / name
        if candidate.exists():
            return candidate
    raise SystemExit(
        "no whycode binary found — build one first:\n"
        "  cargo build --release -p whycode-cli"
    )


def run_once(binary: Path, idle_ms: int, project: Path) -> dict | None:
    """One measured launch. Returns None when the run produced no frame."""
    with tempfile.TemporaryDirectory() as tmp:
        out = Path(tmp) / "bench.json"
        env = {
            **os.environ,
            "WHYCODE_BENCH": str(out),
            "WHYCODE_BENCH_DURATION_MS": str(idle_ms),
            # Keep the run offline and deterministic: no key means no provider
            # call, and the TUI still draws its first frame.
            "ANTHROPIC_API_KEY": "",
            "WHYCODE_AUTO_DENY": "1",
        }
        argv = [str(binary), "run", "-d", str(project)]

        started = time.perf_counter()
        if IS_WINDOWS:
            # Inherit this console; there is no stdlib ConPTY.
            proc = subprocess.run(argv, env=env, timeout=60, check=False)
            code = proc.returncode
        else:
            code = run_in_pty(argv, env)
        elapsed_ms = (time.perf_counter() - started) * 1000.0

        if not out.exists():
            print(f"  run produced no measurement (exit {code})", file=sys.stderr)
            return None
        data = json.loads(out.read_text(encoding="utf-8"))
        data["spawn_to_exit_ms"] = round(elapsed_ms, 3)
        return data


def run_in_pty(argv: list[str], env: dict) -> int:
    """Run under a pty so the TUI sees a terminal, discarding its output."""
    import pty  # noqa: PLC0415 — Unix only

    pid, fd = pty.fork()
    if pid == 0:  # child
        try:
            os.execvpe(argv[0], argv, env)
        finally:
            os._exit(127)

    # Drain, or the child blocks once the pty buffer fills.
    try:
        while True:
            try:
                if not os.read(fd, 65536):
                    break
            except OSError:
                break
    finally:
        os.close(fd)
    _, status = os.waitpid(pid, 0)
    return os.waitstatus_to_exitcode(status)


def summarise(label: str, values: list[float], unit: str = "ms") -> dict:
    values = sorted(values)
    stats = {
        "median": round(statistics.median(values), 2),
        "min": round(values[0], 2),
        "max": round(values[-1], 2),
    }
    print(
        f"{label:<24} {stats['median']:>9.1f}{unit} "
        f"{stats['min']:>9.1f}{unit} {stats['max']:>9.1f}{unit}"
    )
    return stats


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--runs", type=int, default=10)
    parser.add_argument(
        "--idle-ms",
        type=int,
        default=2000,
        help="how long to keep drawing after the first frame, to count idle redraws",
    )
    parser.add_argument("--binary", type=Path, default=None)
    parser.add_argument("--json", type=Path, default=None)
    args = parser.parse_args()

    binary = args.binary or default_binary()
    profile = "release" if "release" in str(binary) else "debug"

    # An empty project, so the measurement is the TUI rather than whatever
    # happens to be in the current directory.
    with tempfile.TemporaryDirectory() as project:
        print(f"{binary}  ({profile}, {platform.system()} {platform.machine()})")
        print(f"{args.runs} runs, {args.idle_ms}ms idle window\n")

        results = []
        # Discard the first run: it pays for reading the binary off disk.
        run_once(binary, args.idle_ms, Path(project))
        for i in range(args.runs):
            result = run_once(binary, args.idle_ms, Path(project))
            if result:
                results.append(result)
            print(f"\r  {i + 1}/{args.runs}", end="", file=sys.stderr)
        print("\r", end="", file=sys.stderr)

    if not results:
        print("every run failed to produce a measurement", file=sys.stderr)
        return 1

    print(f"{'':<24} {'median':>11} {'min':>11} {'max':>11}")
    out = {
        "binary": str(binary),
        "profile": profile,
        "platform": f"{platform.system()} {platform.machine()}",
        "runs": len(results),
        "idle_window_ms": args.idle_ms,
        "first_frame": summarise("first frame (in-proc)", [r["first_frame_ms"] for r in results]),
        "spawn_to_exit": summarise("spawn to exit", [r["spawn_to_exit_ms"] for r in results]),
        "draws_per_second": summarise(
            "idle draws/sec", [r["draws_per_second"] for r in results], unit="/s"
        ),
    }

    idle = out["draws_per_second"]["median"]
    print()
    if idle > 1.0:
        print(
            f"note: the loop redraws ~{idle:.0f} times a second with no input.\n"
            f"      Every one of those repaints an unchanged screen."
        )
    else:
        print("idle redraws are at or near zero.")

    if args.json:
        args.json.write_text(json.dumps(out, indent=2) + "\n", encoding="utf-8")
        print(f"wrote {args.json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
