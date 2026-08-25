#!/usr/bin/env python3
"""Reconcile session usage against the last raw provider usage snapshot.

The 1% acceptance criterion in docs/archive/plan-performance.md: stored
session usage must match the provider's own usage object on a real turn.

  WHYCODES_USAGE_DUMP=/tmp/usage.jsonl whycodes generate --format json ...
  python scripts/reconcile_token_usage.py --dump /tmp/usage.jsonl --result out.json

Or one shot (needs a configured provider):

  python scripts/reconcile_token_usage.py --live

`--self-test` checks the compare math without a network.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

TOLERANCE = 0.01


def tokens_from_usage(usage: dict[str, Any]) -> tuple[int, int]:
    """Read OpenAI (prompt/completion) or Anthropic (input/output) names."""
    inp = usage.get("prompt_tokens")
    if inp is None:
        inp = usage.get("input_tokens", 0)
    out = usage.get("completion_tokens")
    if out is None:
        out = usage.get("output_tokens", 0)
    return int(inp or 0), int(out or 0)


def last_snapshot(dump_lines: list[str]) -> tuple[int, int] | None:
    """Last non-zero raw usage object is the provider total for that step."""
    last: tuple[int, int] | None = None
    for raw in dump_lines:
        raw = raw.strip()
        if not raw:
            continue
        obj = json.loads(raw)
        usage = obj.get("usage") if isinstance(obj, dict) else None
        if not isinstance(usage, dict):
            continue
        inp, out = tokens_from_usage(usage)
        if inp or out:
            last = (inp, out)
    return last


def session_tokens(result: dict[str, Any]) -> tuple[int, int]:
    usage = result.get("usage") or {}
    return tokens_from_usage(usage)


def within_tolerance(a: int, b: int, tol: float = TOLERANCE) -> bool:
    if a == b:
        return True
    denom = max(a, b, 1)
    return abs(a - b) / denom <= tol


def report(
    provider: tuple[int, int], session: tuple[int, int]
) -> tuple[bool, str]:
    ok_in = within_tolerance(provider[0], session[0])
    ok_out = within_tolerance(provider[1], session[1])
    ok = ok_in and ok_out
    lines = [
        f"provider snapshot:  input={provider[0]}  output={provider[1]}",
        f"session stored:     input={session[0]}  output={session[1]}",
        f"input  delta={session[0] - provider[0]}  "
        f"ok={ok_in} (≤{TOLERANCE:.0%})",
        f"output delta={session[1] - provider[1]}  "
        f"ok={ok_out} (≤{TOLERANCE:.0%})",
    ]
    return ok, "\n".join(lines)


def load_result(path: Path | None, text: str | None) -> dict[str, Any]:
    if path is not None:
        text = path.read_text(encoding="utf-8")
    if text is None:
        raise SystemExit("no result JSON")
    # stream-json: last Result object; json: one object.
    last: dict[str, Any] | None = None
    for line in text.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(obj, dict) and "usage" in obj:
            last = obj
    if last is None:
        raise SystemExit("no usage object in generate output")
    return last


def run_self_test() -> int:
    # Last snapshot wins; earlier running totals are ignored.
    dump = [
        json.dumps({"source": "openai_compat", "usage": {"prompt_tokens": 10, "completion_tokens": 1}}),
        json.dumps({"source": "openai_compat", "usage": {"prompt_tokens": 10, "completion_tokens": 4}}),
    ]
    snap = last_snapshot(dump)
    assert snap == (10, 4), snap
    assert within_tolerance(100, 100)
    assert within_tolerance(100, 101)  # 1%
    assert not within_tolerance(100, 103)
    assert tokens_from_usage({"input_tokens": 7, "output_tokens": 2}) == (7, 2)
    print("self-test ok")
    return 0


def run_live(whycodes: Path, prompt: str, project: Path | None) -> int:
    dump_path = Path(tempfile.mkstemp(prefix="whycodes-usage-", suffix=".jsonl")[1])
    env = os.environ.copy()
    env["WHYCODES_USAGE_DUMP"] = str(dump_path)
    env.setdefault("WHYCODES_AUTO_APPROVE", "1")
    cmd = [str(whycodes), "--no-memory", "-a", "ask"]
    if project is not None:
        cmd.extend(["-d", str(project)])
    cmd.extend(
        [
            "generate",
            "--format",
            "json",
            "-t",
            "1",
            prompt,
        ]
    )
    print("running:", " ".join(cmd), file=sys.stderr)
    proc = subprocess.run(
        cmd,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        print(proc.stdout)
        return proc.returncode or 1
    dump_text = dump_path.read_text(encoding="utf-8") if dump_path.exists() else ""
    dump_path.unlink(missing_ok=True)
    return compare(dump_text.splitlines(), proc.stdout)


def compare(dump_lines: list[str], result_text: str) -> int:
    snap = last_snapshot(dump_lines)
    if snap is None:
        print("no raw provider usage dumped — is WHYCODES_USAGE_DUMP set?", file=sys.stderr)
        return 2
    result = load_result(None, result_text)
    session = session_tokens(result)
    ok, text = report(snap, session)
    print(text)
    if result.get("provider"):
        print(f"provider={result.get('provider')} model={result.get('model')}")
    return 0 if ok else 1


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--dump", type=Path, help="WHYCODES_USAGE_DUMP JSONL")
    ap.add_argument("--result", type=Path, help="generate --format json output")
    ap.add_argument("--live", action="store_true", help="run one generate turn")
    ap.add_argument(
        "--whycodes",
        type=Path,
        default=Path("target/debug/whycodes"),
        help="whycodes binary for --live",
    )
    ap.add_argument(
        "--prompt",
        default="Reply with exactly: pong",
        help="prompt for --live",
    )
    ap.add_argument(
        "--project",
        type=Path,
        help="project dir for --live (`-d`; use to load .whycodes/config.toml)",
    )
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args()

    if args.self_test:
        return run_self_test()
    if args.live:
        if not args.whycodes.exists():
            print(f"missing binary {args.whycodes} — cargo build -p whycodes-cli", file=sys.stderr)
            return 2
        return run_live(args.whycodes, args.prompt, args.project)
    if args.dump is None or args.result is None:
        ap.error("need --live, --self-test, or both --dump and --result")
    return compare(
        args.dump.read_text(encoding="utf-8").splitlines(),
        args.result.read_text(encoding="utf-8"),
    )


if __name__ == "__main__":
    raise SystemExit(main())
