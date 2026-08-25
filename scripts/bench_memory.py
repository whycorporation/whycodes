#!/usr/bin/env python3
"""Measure memory for whycodes: peak RSS of short CLI runs, and multi-session PSS.

Two things are measured, for different reasons:

  Peak RSS  short-lived subcommands (`--version`, `config show`, …). Samples
            while the process runs. What a one-shot invocation costs.

  PSS       N concurrent idle TUI sessions held open together. Proportional
            Set Size from `/proc/<pid>/smaps_rollup` so shared pages are not
            counted N times. This is the figure jcode and other harnesses
            publish for "10 session" comparisons.

PSS needs Linux. On other platforms the multi-session section is skipped with
a note; peak RSS still runs.

Requires `psutil` for peak RSS. Without it the script says so and exits 0
rather than failing: this is a reporting tool, and a missing optional
dependency should not break a pipeline that also runs the budgets.

Usage:
    python scripts/bench_memory.py [--runs 5] [--sessions 1 10]
                                   [--settle 1.5] [--binary path] [--json out.json]
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import signal
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
IS_LINUX = platform.system() == "Linux"
IS_WINDOWS = platform.system() == "Windows"

CASES = [
    ("version", ["--version"]),
    ("config-show", ["config", "show"]),
    ("session-list", ["session", "list"]),
]

# How often to sample RSS. Short-lived processes need a tight loop or the peak
# is missed entirely; this is the trade-off between accuracy and burning CPU.
SAMPLE_INTERVAL_S = 0.001

# Keep the TUI drawing well past settle so PSS is sampled on a live session,
# not during teardown. The harness kills the process group after the sample.
SESSION_HOLD_MS = 60_000


def default_binary() -> Path:
    name = "whycodes.exe" if IS_WINDOWS else "whycodes"
    for profile in ("release", "debug"):
        candidate = ROOT / "target" / profile / name
        if candidate.exists():
            return candidate
    raise SystemExit(
        "no whycodes binary found — build one first:\n  cargo build --release -p whycodes-cli"
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


# ── Multi-session PSS (Linux) ──────────────────────────────────────────────


def read_pss_kb(pid: int) -> int | None:
    """Proportional set size in KiB from smaps_rollup, or None if gone/unreadable."""
    path = Path(f"/proc/{pid}/smaps_rollup")
    try:
        for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
            if line.startswith("Pss:"):
                return int(line.split()[1])
    except (OSError, ValueError, IndexError):
        return None
    return None


def iter_proc_ppid_pgid() -> dict[int, tuple[int, int]]:
    """pid → (ppid, pgid) from /proc/*/stat."""
    out: dict[int, tuple[int, int]] = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            stat = (entry / "stat").read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        try:
            # comm can contain spaces/parens; fields after the last ')' are fixed.
            close = stat.rfind(")")
            rest = stat[close + 2 :].split()
            ppid = int(rest[1])
            pgid = int(rest[2])
            out[int(entry.name)] = (ppid, pgid)
        except (ValueError, IndexError):
            continue
    return out


def collect_tree_pids(root_pids: list[int], pgids: list[int]) -> set[int]:
    """Root PIDs, their descendants, and every member of the process groups."""
    ppid_of = iter_proc_ppid_pgid()
    children: dict[int, list[int]] = {}
    for pid, (ppid, _pgid) in ppid_of.items():
        children.setdefault(ppid, []).append(pid)

    seen: set[int] = set()
    stack = list(root_pids)
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        seen.add(pid)
        stack.extend(children.get(pid, []))

    wanted_pgids = set(pgids)
    for pid, (_ppid, pgid) in ppid_of.items():
        if pgid in wanted_pgids:
            seen.add(pid)
    return seen


def sum_tree_pss_mb(root_pids: list[int], pgids: list[int]) -> tuple[float, int]:
    total_kb = 0
    counted = 0
    for pid in sorted(collect_tree_pids(root_pids, pgids)):
        pss = read_pss_kb(pid)
        if pss is None:
            continue
        total_kb += pss
        counted += 1
    return round(total_kb / 1024.0, 1), counted


def terminate_pgroup(pgid: int, pid: int | None = None) -> None:
    """SIGTERM then SIGKILL the process group (and the root pid as a fallback)."""
    for sig in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(pgid, sig)
        except (ProcessLookupError, PermissionError):
            pass
        if pid is not None:
            try:
                os.kill(pid, sig)
            except (ProcessLookupError, PermissionError):
                pass
        time.sleep(0.1 if sig == signal.SIGTERM else 0.05)


def session_env(home: Path, project: Path) -> dict[str, str]:
    """Isolated env so the bench never touches the developer's real data dir."""
    env = os.environ.copy()
    env["HOME"] = str(home)
    env["XDG_CONFIG_HOME"] = str(home / ".config")
    env["XDG_DATA_HOME"] = str(home / ".local" / "share")
    env["XDG_STATE_HOME"] = str(home / ".local" / "state")
    env["XDG_CACHE_HOME"] = str(home / ".cache")
    # Offline, deterministic: no provider call; TUI still draws its first frame.
    env["ANTHROPIC_API_KEY"] = ""
    env["OPENAI_API_KEY"] = ""
    env["WHYCODES_AUTO_DENY"] = "1"
    # Hold the session open long enough to sample; harness kills after.
    env["WHYCODES_BENCH"] = str(home / "bench-hold.json")
    env["WHYCODES_BENCH_DURATION_MS"] = str(SESSION_HOLD_MS)
    # Avoid inheriting a weird cwd-dependent project path for config layering.
    env["PWD"] = str(project)
    return env


# Terminal queries the TUI emits at startup (crossterm / ratatui). Without
# replies the process sits blocked and never paints — memory stays at the
# pre-frame floor and is not comparable to a real idle session.
_TERMINAL_REPLIES: list[tuple[bytes, bytes]] = [
    (b"\x1b[6n", b"\x1b[1;1R"),
    (b"\x1b[c", b"\x1b[?62;c"),
    (b"\x1b]10;?\x1b\\", b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\"),
    (b"\x1b]11;?\x1b\\", b"\x1b]11;rgb:0000/0000/0000\x1b\\"),
    (b"\x1b]10;?\x07", b"\x1b]10;rgb:ffff/ffff/ffff\x07"),
    (b"\x1b]11;?\x07", b"\x1b]11;rgb:0000/0000/0000\x07"),
    (b"\x1b]4;0;?\x07", b"\x1b]4;0;rgb:0000/0000/0000\x07"),
    (b"\x1b[14t", b"\x1b[4;600;800t"),
    (b"\x1b[16t", b"\x1b[6;16;8t"),
    (b"\x1b[18t", b"\x1b[8;24;80t"),
    (b"\x1b[?1016$p", b"\x1b[?1016;1$y"),
    (b"\x1b[?2027$p", b"\x1b[?2027;1$y"),
    (b"\x1b[?2031$p", b"\x1b[?2031;1$y"),
    (b"\x1b[?1004$p", b"\x1b[?1004;1$y"),
    (b"\x1b[?2004$p", b"\x1b[?2004;1$y"),
    (b"\x1b[?2026$p", b"\x1b[?2026;1$y"),
]


def reply_terminal_queries(master_fd: int, buf: bytes) -> bytes:
    """Answer capability probes so the TUI proceeds past startup."""
    changed = True
    while changed:
        changed = False
        for query, response in _TERMINAL_REPLIES:
            if query in buf:
                try:
                    os.write(master_fd, response)
                except OSError:
                    return buf
                buf = buf.replace(query, b"")
                changed = True
    return buf


def launch_session(
    binary: Path, project: Path, home: Path
) -> tuple[subprocess.Popen, int, int]:
    """Start one TUI session under a pty. Returns (proc, pgid, master_fd)."""
    import pty  # noqa: PLC0415 — Unix only

    argv = [str(binary), "run", "-d", str(project)]
    env = session_env(home, project)

    master_fd, slave_fd = pty.openpty()
    proc = subprocess.Popen(
        argv,
        cwd=str(project),
        env=env,
        stdin=slave_fd,
        stdout=slave_fd,
        stderr=slave_fd,
        start_new_session=True,
    )
    os.close(slave_fd)
    try:
        os.set_blocking(master_fd, False)
    except OSError:
        pass
    try:
        pgid = os.getpgid(proc.pid)
    except ProcessLookupError:
        pgid = proc.pid
    return proc, pgid, master_fd


def service_pty(master_fd: int, buf: bytearray) -> None:
    """Drain pty output and answer any terminal queries sitting in it."""
    try:
        while True:
            try:
                chunk = os.read(master_fd, 65536)
            except BlockingIOError:
                break
            except OSError:
                break
            if not chunk:
                break
            buf.extend(chunk)
    except OSError:
        return
    remaining = reply_terminal_queries(master_fd, bytes(buf))
    buf.clear()
    buf.extend(remaining)


def multi_session_pss_mb(
    binary: Path, n_sessions: int, settle_s: float
) -> dict:
    """Spawn N idle TUI sessions, settle, sum tree PSS, tear down."""
    with tempfile.TemporaryDirectory(prefix="whycodes-pss-") as tmp:
        root = Path(tmp)
        project = root / "project"
        project.mkdir()
        # Minimal project file so the TUI has something to open against.
        (project / "README.md").write_text("# bench\n", encoding="utf-8")

        # Shared XDG home: same data dir across sessions, like a user with
        # N terminals on one machine. Per-session homes would understate
        # shared-page cost and overstate private cost.
        home = root / "home"
        home.mkdir(exist_ok=True)

        launches: list[tuple[subprocess.Popen, int, int, bytearray]] = []
        try:
            for _ in range(n_sessions):
                proc, pgid, master_fd = launch_session(binary, project, home)
                launches.append((proc, pgid, master_fd, bytearray()))

            # Answer queries and wait. First frame is a few ms once probes
            # are answered; settle is the idle baseline the comparison table
            # uses, not a readiness probe.
            end = time.monotonic() + settle_s
            while time.monotonic() < end:
                for _proc, _pgid, master_fd, buf in launches:
                    service_pty(master_fd, buf)
                time.sleep(0.05)
            for _proc, _pgid, master_fd, buf in launches:
                service_pty(master_fd, buf)

            root_pids = [proc.pid for proc, _, _, _ in launches if proc.poll() is None]
            pgids = [
                pgid
                for proc, pgid, _, _ in launches
                if proc.poll() is None
            ]
            if len(root_pids) < n_sessions:
                dead = n_sessions - len(root_pids)
                raise RuntimeError(
                    f"{dead}/{n_sessions} session(s) exited before PSS sample"
                )

            pss_mb, process_count = sum_tree_pss_mb(root_pids, pgids)
            return {
                "sessions": n_sessions,
                "pss_mb": pss_mb,
                "process_count": process_count,
                "alive": len(root_pids),
            }
        finally:
            for proc, pgid, master_fd, _buf in launches:
                try:
                    os.close(master_fd)
                except OSError:
                    pass
                terminate_pgroup(pgid, proc.pid)
                try:
                    proc.wait(timeout=2)
                except Exception:  # noqa: BLE001
                    try:
                        proc.kill()
                    except Exception:  # noqa: BLE001
                        pass


def run_peak_rss(psutil, binary: Path, runs: int) -> dict:
    cases = {}
    print(f"{'case':<14} {'median':>10} {'max':>10}")
    for label, argv in CASES:
        samples = sorted(peak_rss_mb(psutil, binary, argv) for _ in range(runs))
        stats = {
            "runs": runs,
            "median_mb": round(statistics.median(samples), 2),
            "max_mb": samples[-1],
        }
        cases[label] = stats
        print(f"{label:<14} {stats['median_mb']:>9.1f}M {stats['max_mb']:>9.1f}M")
    return cases


def run_multi_session(
    binary: Path, session_counts: list[int], settle_s: float, runs: int
) -> dict:
    if not IS_LINUX:
        print(
            "\nmulti-session PSS: skipped (needs Linux /proc/.../smaps_rollup)"
        )
        return {"skipped": True, "reason": "not-linux"}

    print(f"\n{'sessions':<14} {'median PSS':>12} {'max PSS':>10} {'procs':>7}")
    out: dict = {"settle_s": settle_s, "counts": {}}
    for n in session_counts:
        samples: list[float] = []
        proc_counts: list[int] = []
        for _ in range(runs):
            result = multi_session_pss_mb(binary, n, settle_s)
            samples.append(result["pss_mb"])
            proc_counts.append(result["process_count"])
        samples.sort()
        stats = {
            "runs": runs,
            "sessions": n,
            "median_pss_mb": round(statistics.median(samples), 1),
            "max_pss_mb": samples[-1],
            "min_pss_mb": samples[0],
            "median_process_count": int(statistics.median(proc_counts)),
            "samples_mb": samples,
        }
        out["counts"][str(n)] = stats
        print(
            f"{n:<14} {stats['median_pss_mb']:>11.1f}M "
            f"{stats['max_pss_mb']:>9.1f}M "
            f"{stats['median_process_count']:>7}"
        )

    # Per-session increment from 1 → N when both are present.
    counts = out["counts"]
    if "1" in counts and any(k != "1" for k in counts):
        one = counts["1"]["median_pss_mb"]
        print()
        for key, stats in counts.items():
            n = int(key)
            if n <= 1:
                continue
            extra = stats["median_pss_mb"] - one
            per = extra / (n - 1) if n > 1 else 0.0
            stats["extra_over_one_mb"] = round(extra, 1)
            stats["per_added_session_mb"] = round(per, 1)
            print(
                f"  {n} sessions: +{extra:.1f}M over one "
                f"(~{per:.1f}M per added session)"
            )
    return out


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Peak RSS + multi-session PSS memory benchmark"
    )
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument(
        "--sessions",
        type=int,
        nargs="+",
        default=[1, 10],
        help="concurrent idle TUI session counts to measure (default: 1 10)",
    )
    parser.add_argument(
        "--settle",
        type=float,
        default=1.5,
        help="seconds to wait after spawning sessions before sampling PSS",
    )
    parser.add_argument(
        "--skip-cli",
        action="store_true",
        help="skip short-lived peak-RSS cases; only multi-session PSS",
    )
    parser.add_argument(
        "--skip-sessions",
        action="store_true",
        help="skip multi-session PSS; only peak RSS",
    )
    parser.add_argument("--binary", type=Path, default=None)
    parser.add_argument("--json", type=Path, default=None)
    args = parser.parse_args()

    try:
        import psutil  # noqa: PLC0415
    except ImportError:
        if args.skip_cli and not args.skip_sessions and IS_LINUX:
            # Multi-session PSS does not need psutil.
            psutil = None  # type: ignore[assignment]
        else:
            print("psutil is not installed; skipping memory benchmark")
            print("  pip install psutil")
            return 0

    # Sessions launch with cwd=project; a relative binary path would miss.
    binary = (args.binary or default_binary()).resolve()
    profile = "release" if "release" in str(binary) else "debug"

    results: dict = {
        "binary": str(binary),
        "profile": profile,
        "platform": f"{platform.system()} {platform.machine()}",
        "cases": {},
        "multi_session": {},
    }

    print(f"{binary}  ({profile}, {results['platform']})")

    if not args.skip_cli:
        if psutil is None:
            print("psutil is not installed; skipping peak-RSS cases")
            print("  pip install psutil")
        else:
            results["cases"] = run_peak_rss(psutil, binary, args.runs)

    if not args.skip_sessions:
        # One warm-up spawn so the binary is in the page cache before the
        # first timed multi-session sample (mirrors bench_startup / first_frame).
        if IS_LINUX:
            try:
                multi_session_pss_mb(binary, 1, min(0.5, args.settle))
            except Exception as exc:  # noqa: BLE001 — warm-up must not abort the run
                print(f"  warm-up session note: {exc}", file=sys.stderr)
        results["multi_session"] = run_multi_session(
            binary, args.sessions, args.settle, args.runs
        )

    if args.json:
        args.json.write_text(json.dumps(results, indent=2) + "\n", encoding="utf-8")
        print(f"\nwrote {args.json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
