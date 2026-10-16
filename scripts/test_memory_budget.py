#!/usr/bin/env python3
"""Offline checks for scripts/check_memory_budget.py."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CHECK = ROOT / "scripts" / "check_memory_budget.py"
SOURCE = "crates/demo/src/cache.rs"


def run_check(root: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(CHECK), "--root", str(root)],
        text=True,
        capture_output=True,
    )


def make_tree(tmp: Path, source: str, value: int = 64 * 1024) -> None:
    src = tmp / SOURCE
    src.parent.mkdir(parents=True)
    src.write_text(source, encoding="utf-8")
    (tmp / "scripts").mkdir()
    budget = {
        "bounds": [{"file": SOURCE, "const": "MAX_BYTES", "value": value}],
    }
    (tmp / "scripts" / "memory_budget.json").write_text(
        json.dumps(budget), encoding="utf-8"
    )


def expect(source: str, ok: bool, needle: str, value: int = 64 * 1024) -> None:
    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)
        make_tree(tmp, source, value)
        proc = run_check(tmp)
        out = proc.stdout + proc.stderr
        if (proc.returncode == 0) != ok:
            raise SystemExit(f"expected ok={ok} for {source!r}:\n{out}")
        if needle not in out:
            raise SystemExit(f"missing {needle!r} for {source!r}:\n{out}")


def test_current_repo_passes() -> None:
    proc = subprocess.run([sys.executable, str(CHECK)], text=True, capture_output=True)
    if proc.returncode != 0 or "memory budget: ok" not in proc.stdout:
        raise SystemExit(f"current repo should pass:\n{proc.stdout}{proc.stderr}")


def test_equal_value_passes() -> None:
    expect("pub const MAX_BYTES: usize = 64 * 1024;\n", True, "memory budget: ok")


def test_underscored_literal_passes() -> None:
    expect("    const MAX_BYTES: u64 = 65_536;\n", True, "memory budget: ok")


def test_growth_fails() -> None:
    expect("const MAX_BYTES: usize = 128 * 1024;\n", False, "budget 65536")


def test_shrink_reports_slack() -> None:
    expect("pub(crate) const MAX_BYTES: usize = 32 * 1024;\n", True, "Below budget")


def test_missing_const_fails() -> None:
    expect("const OTHER: usize = 1;\n", False, "const MAX_BYTES not found")


def test_missing_file_fails() -> None:
    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)
        make_tree(tmp, "const MAX_BYTES: usize = 1;\n")
        (tmp / SOURCE).unlink()
        proc = run_check(tmp)
        if proc.returncode == 0 or "file not found" not in proc.stdout:
            raise SystemExit(f"expected missing file to fail:\n{proc.stdout}")


def test_non_literal_fails() -> None:
    expect("const MAX_BYTES: usize = OTHER * 2;\n", False, "is not a plain number")


def test_duplicate_definition_fails() -> None:
    src = "const MAX_BYTES: usize = 1;\nconst MAX_BYTES: usize = 2;\n"
    expect(src, False, "defined more than once")


def test_duplicate_entry_fails() -> None:
    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)
        make_tree(tmp, "const MAX_BYTES: usize = 1;\n", 1)
        path = tmp / "scripts" / "memory_budget.json"
        data = json.loads(path.read_text(encoding="utf-8"))
        data["bounds"].append(dict(data["bounds"][0]))
        path.write_text(json.dumps(data), encoding="utf-8")
        proc = run_check(tmp)
        if proc.returncode == 0 or "twice" not in proc.stdout:
            raise SystemExit(f"expected duplicate entry to fail:\n{proc.stdout}")


def main() -> int:
    test_current_repo_passes()
    test_equal_value_passes()
    test_underscored_literal_passes()
    test_growth_fails()
    test_shrink_reports_slack()
    test_missing_const_fails()
    test_missing_file_fails()
    test_non_literal_fails()
    test_duplicate_definition_fails()
    test_duplicate_entry_fails()
    print("test_memory_budget: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
