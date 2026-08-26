#!/usr/bin/env python3
"""Offline checks for scripts/check_tracked_secrets.py."""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CHECK = ROOT / "scripts" / "check_tracked_secrets.py"


def run_check(cwd: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(CHECK)],
        cwd=cwd,
        text=True,
        capture_output=True,
    )


def git(cwd: Path, *args: str) -> None:
    subprocess.run(
        ["git", *args],
        cwd=cwd,
        check=True,
        capture_output=True,
        text=True,
    )


def init_repo(tmp: Path) -> None:
    git(tmp, "init", "-q")
    git(tmp, "config", "user.email", "dev@example.com")
    git(tmp, "config", "user.name", "dev")
    (tmp / "README").write_text("ok\n", encoding="utf-8")
    git(tmp, "add", "README")
    git(tmp, "commit", "-q", "-m", "init")


def test_current_repo_passes() -> None:
    proc = run_check(ROOT)
    if proc.returncode != 0:
        raise SystemExit(
            f"current repo should pass:\n{proc.stdout}{proc.stderr}"
        )
    if "ok" not in proc.stdout:
        raise SystemExit(f"expected ok line, got:\n{proc.stdout}")


def test_scratch_dir_rejected() -> None:
    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)
        init_repo(tmp)
        omo = tmp / ".omo" / "ses.json"
        omo.parent.mkdir()
        omo.write_text("{}\n", encoding="utf-8")
        git(tmp, "add", "-f", ".omo/ses.json")
        proc = run_check(tmp)
        if proc.returncode == 0:
            raise SystemExit("expected .omo/ to fail")
        if ".omo/ses.json" not in proc.stdout:
            raise SystemExit(f"missing path in output:\n{proc.stdout}")


def test_live_pat_rejected() -> None:
    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)
        init_repo(tmp)
        (tmp / "leak.txt").write_text(
            "token = ghp_" + ("A" * 36) + "\n", encoding="utf-8"
        )
        git(tmp, "add", "leak.txt")
        proc = run_check(tmp)
        if proc.returncode == 0:
            raise SystemExit("expected ghp_ PAT to fail")
        if "GitHub PAT" not in proc.stdout:
            raise SystemExit(f"missing PAT label:\n{proc.stdout}")


def test_env_example_allowed() -> None:
    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)
        init_repo(tmp)
        (tmp / ".env.example").write_text("ANTHROPIC_API_KEY=\n", encoding="utf-8")
        git(tmp, "add", ".env.example")
        proc = run_check(tmp)
        if proc.returncode != 0:
            raise SystemExit(
                f".env.example should pass:\n{proc.stdout}{proc.stderr}"
            )


def test_shareable_whycodes_commands_allowed() -> None:
    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)
        init_repo(tmp)
        dest = tmp / ".whycodes" / "commands" / "demo.md"
        dest.parent.mkdir(parents=True)
        dest.write_text("# demo\n", encoding="utf-8")
        git(tmp, "add", "-f", ".whycodes/commands/demo.md")
        proc = run_check(tmp)
        if proc.returncode != 0:
            raise SystemExit(
                f".whycodes/commands should pass:\n{proc.stdout}{proc.stderr}"
            )


def test_whycodes_todos_rejected() -> None:
    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)
        init_repo(tmp)
        dest = tmp / ".whycodes" / "todos" / "x.json"
        dest.parent.mkdir(parents=True)
        dest.write_text("{}\n", encoding="utf-8")
        git(tmp, "add", "-f", ".whycodes/todos/x.json")
        proc = run_check(tmp)
        if proc.returncode == 0:
            raise SystemExit("expected .whycodes/todos to fail")


def main() -> int:
    os.environ.setdefault("GIT_AUTHOR_DATE", "2026-01-01T00:00:00")
    os.environ.setdefault("GIT_COMMITTER_DATE", "2026-01-01T00:00:00")
    test_current_repo_passes()
    test_scratch_dir_rejected()
    test_live_pat_rejected()
    test_env_example_allowed()
    test_shareable_whycodes_commands_allowed()
    test_whycodes_todos_rejected()
    print("test_tracked_secrets: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
