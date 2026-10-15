#!/usr/bin/env python3
"""Memory of a TUI that resumed a long, code-heavy session.

`bench_memory.py` measures idle, empty TUIs. This one measures the case that
costs memory: a long transcript with fenced code in many languages, resumed
and painted. It writes a synthetic Claude-style JSONL transcript, imports it
into an isolated home with `whycodes session import`, opens the TUI on it
under a PTY, and reads PSS (`/proc/<pid>/smaps_rollup`) and `RssAnon`
(`/proc/<pid>/status`):

  open   after the first paint has settled
  idle   after the client has been idle long enough to return retained heap

Linux only. No provider key or network is needed: the transcript is local and
no turn runs.

Usage:
    python scripts/bench_resume_memory.py --binary target/release/whycodes
        [--binary other] [--turns 1500] [--settle 8] [--idle 75] [--runs 3]
        [--json out.json]
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import signal
import statistics
import struct
import subprocess
import sys
import tempfile
import time

SNIPPETS = {
    "rust": "fn main() {\n    let v: Vec<u32> = (0..10).map(|x| x * x).collect();\n    println!(\"{:?}\", v);\n}",
    "python": "def f(xs):\n    return [x * x for x in xs if x % 2]\n\nprint(f(range(10)))",
    "javascript": "const f = async (u) => {\n  const r = await fetch(u);\n  return r?.json() ?? {};\n};",
    "typescript": "export function g<T extends object>(x: T): T {\n  return { ...x } as T;\n}",
    "go": "package main\n\nimport \"fmt\"\n\nfunc main() {\n\tfmt.Println(\"hi\")\n}",
    "c": "#include <stdio.h>\nint main(void) {\n    printf(\"%d\\n\", 42);\n    return 0;\n}",
    "cpp": "#include <vector>\ntemplate <class T> T sum(const std::vector<T>& v) {\n  T s{};\n  for (auto& x : v) s += x;\n  return s;\n}",
    "java": "public class A {\n  public static void main(String[] a) {\n    System.out.println(\"x\");\n  }\n}",
    "bash": "for f in *.rs; do\n  echo \"$f\" | wc -c\ndone",
    "json": "{\n  \"a\": [1, 2, {\"b\": null}],\n  \"c\": true\n}",
    "yaml": "jobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo test",
    "html": "<div class=\"a\">\n  <p>hello <b>world</b></p>\n</div>",
    "css": ".a > .b:hover {\n  color: #ff0000;\n  margin: 0 auto;\n}",
    "sql": "SELECT a, count(*)\nFROM t\nWHERE b > 1\nGROUP BY a;",
    "ruby": "class A\n  def f(x)\n    x.map { |y| y * 2 }\n  end\nend",
    "php": "<?php\nfunction f($x) {\n  return array_map(fn($y) => $y * 2, $x);\n}",
}

UUID = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")


def write_transcript(path: str, turns: int) -> None:
    langs = list(SNIPPETS)
    prose = " ".join(["This paragraph explains the example in some detail."] * 6)
    with open(path, "w", encoding="utf-8") as f:
        for i in range(turns):
            lang = langs[i % len(langs)]
            body = "\n".join([SNIPPETS[lang]] * 6)
            text = (
                f"Here is example {i}.\n\n{prose}\n\n```{lang}\n{body}\n```\n\n"
                f"{prose}\n\n- point one\n- point two\n"
            )
            user = {"role": "user", "content": f"Show me example {i} in {lang}."}
            assistant = {"role": "assistant", "content": [{"type": "text", "text": text}]}
            f.write(json.dumps({"type": "user", "message": user}) + "\n")
            f.write(json.dumps({"type": "assistant", "message": assistant}) + "\n")


def read_kb(path: str, key: str) -> float | None:
    try:
        with open(path, encoding="utf-8") as fh:
            for line in fh:
                if line.startswith(key):
                    return int(line.split()[1]) / 1024
    except OSError:
        return None
    return None


def sample(pid: int) -> dict[str, float | None]:
    return {
        "pss_mb": read_kb(f"/proc/{pid}/smaps_rollup", "Pss:"),
        "rss_anon_mb": read_kb(f"/proc/{pid}/status", "RssAnon:"),
    }


def pump(fd: int, seconds: float) -> None:
    """Read the PTY for `seconds`, answering cursor-position queries."""
    import select

    until = time.time() + seconds
    while time.time() < until:
        ready, _, _ = select.select([fd], [], [], 0.1)
        if not ready:
            continue
        try:
            data = os.read(fd, 65536)
        except OSError:
            return
        if b"\x1b[6n" in data:
            os.write(fd, b"\x1b[1;1R")


def run_once(binary: str, transcript: str, settle: float, idle: float) -> dict:
    import fcntl
    import pty
    import termios

    home = tempfile.mkdtemp(prefix="whycodes-resume-")
    project = os.path.join(home, "proj")
    os.makedirs(project)
    env = dict(
        os.environ,
        HOME=home,
        WHYCODES_HOME=os.path.join(home, ".whycodes"),
        XDG_CONFIG_HOME=os.path.join(home, ".config"),
        XDG_DATA_HOME=os.path.join(home, ".local", "share"),
        TERM="xterm-256color",
    )
    try:
        imported = subprocess.run(
            [binary, "session", "import", transcript, "-d", project],
            env=env, capture_output=True, text=True, check=False,
        )
        ids = UUID.findall(imported.stdout + imported.stderr)
        if not ids:
            return {"error": (imported.stdout + imported.stderr)[-300:]}
        pid, fd = pty.fork()
        if pid == 0:
            os.chdir(project)
            os.execvpe(binary, [binary, "-r", ids[0], "--no-auto-update", "-d", project], env)
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
        pump(fd, settle)
        result = {"open": sample(pid)}
        if idle > 0:
            pump(fd, idle)
            result["idle"] = sample(pid)
        os.kill(pid, signal.SIGKILL)
        os.waitpid(pid, 0)
        return result
    finally:
        shutil.rmtree(home, ignore_errors=True)


def median(runs: list[dict], phase: str, key: str) -> float | None:
    values = [r[phase][key] for r in runs if phase in r and r[phase][key] is not None]
    return statistics.median(values) if values else None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--binary", action="append", required=True)
    parser.add_argument("--turns", type=int, default=1500, help="user+assistant pairs")
    parser.add_argument("--settle", type=float, default=8.0)
    parser.add_argument("--idle", type=float, default=75.0, help="0 skips the idle sample")
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--json", default=None)
    args = parser.parse_args()
    if not sys.platform.startswith("linux"):
        print("bench_resume_memory: needs Linux (/proc PSS); skipped")
        return 0

    transcript = os.path.join(tempfile.mkdtemp(prefix="whycodes-transcript-"), "t.jsonl")
    write_transcript(transcript, args.turns)
    size_kib = os.path.getsize(transcript) / 1024
    print(f"transcript: {2 * args.turns} messages, {size_kib:.0f} KiB")

    report = {"turns": args.turns, "transcript_kib": round(size_kib), "binaries": {}}
    for binary in args.binary:
        runs = [run_once(binary, transcript, args.settle, args.idle) for _ in range(args.runs)]
        errors = [r["error"] for r in runs if "error" in r]
        if errors:
            print(f"{binary}: ERROR {errors[0]}")
            continue
        summary = {
            f"{phase}_{key}": median(runs, phase, key)
            for phase in ("open", "idle")
            for key in ("pss_mb", "rss_anon_mb")
        }
        report["binaries"][binary] = {"median": summary, "runs": runs}
        shown = "  ".join(f"{k}={v:.1f}" for k, v in summary.items() if v is not None)
        print(f"{binary}: {shown}")
    if args.json:
        with open(args.json, "w", encoding="utf-8") as fh:
            json.dump(report, fh, indent=2)
    return 0


if __name__ == "__main__":
    sys.exit(main())
