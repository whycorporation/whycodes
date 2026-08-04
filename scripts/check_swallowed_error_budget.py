#!/usr/bin/env python3
"""Fail when a crate gains places that discard an error.

Counts three shapes in non-test code:

  let _ = something(...)   a call whose Result is thrown away
  Err(_) => ...            an error matched without being named
  ...ok();                 a Result converted to Option and dropped

This is deliberately crude. Some hits are legitimate — best-effort cleanup in a
test teardown, a pragma that is allowed to fail. The budget does not try to
judge each one; it makes the total visible and stops it drifting upward
unnoticed.

Motivating case, from this repository: `cmd_stats` swallowed a database error
that `cmd_session` propagated. That asymmetry is why a macOS CI failure took
three rounds to diagnose — one command reported the fault and the other hid it.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BUDGET_FILE = Path(__file__).resolve().parent / "swallowed_error_budget.json"

PATTERNS = [
    # `let _ = call(...)` — the underscore binding exists to silence a Result.
    (re.compile(r"\blet\s+_\s*=\s*[^;]*\("), "discarded result"),
    # An error arm that does not bind the error.
    (re.compile(r"\bErr\(_\)\s*=>"), "unnamed Err arm"),
    # `.ok();` at the end of a statement drops the error.
    (re.compile(r"\.ok\(\)\s*;"), "Result dropped via .ok()"),
]

CFG_TEST = re.compile(r"^\s*#\[cfg\(test\)\]")


def strip_test_code(lines: list[str]) -> list[tuple[int, str]]:
    """Return (line number, text) for lines outside `#[cfg(test)]` blocks.

    `#[cfg(test)] mod foo;` (semicolon, body in another file) only skips that
    declaration — it must not put the rest of the current file into skip mode.
    """
    out: list[tuple[int, str]] = []
    skipping = False
    depth = 0
    pending = False

    for number, line in enumerate(lines, start=1):
        if not skipping and CFG_TEST.match(line):
            pending = True
            continue
        if pending:
            # External module: `mod tests;` — body lives elsewhere, not here.
            if "{" not in line and ";" in line:
                pending = False
                continue
            depth += line.count("{") - line.count("}")
            if "{" in line:
                pending = False
                skipping = True
            continue
        if skipping:
            depth += line.count("{") - line.count("}")
            if depth <= 0:
                skipping = False
                depth = 0
            continue
        out.append((number, line))
    return out


def is_test_path(path: Path) -> bool:
    """True for integration-test trees and unit-test module files.

    `path.parts` only matches a directory named `tests`, so `src/tests.rs`
    (the usual `#[cfg(test)] mod tests;` body) must be excluded by filename.
    """
    if "tests" in path.parts:
        return True
    name = path.name
    return name in ("tests.rs", "test.rs") or name.endswith("_tests.rs")


def count_crate(crate: Path) -> list[tuple[Path, int, str]]:
    findings: list[tuple[Path, int, str]] = []
    for path in sorted((crate / "src").rglob("*.rs")):
        if is_test_path(path):
            continue
        try:
            lines = path.read_text(encoding="utf-8").splitlines()
        except (OSError, UnicodeDecodeError):
            continue
        for number, line in strip_test_code(lines):
            code = line.split("//", 1)[0]
            for pattern, label in PATTERNS:
                if pattern.search(code):
                    findings.append((path.relative_to(ROOT), number, label))
    return findings


def main() -> int:
    budgets = json.loads(BUDGET_FILE.read_text(encoding="utf-8"))
    crates = sorted(p for p in (ROOT / "crates").iterdir() if (p / "src").is_dir())

    failed = False
    slack: list[str] = []

    for crate in crates:
        name = crate.name
        findings = count_crate(crate)
        count = len(findings)
        budget = budgets.get(name)

        if budget is None:
            print(f"error: crate '{name}' has no entry in {BUDGET_FILE.name}")
            failed = True
            continue

        if count > budget:
            failed = True
            print(f"\n{name}: {count} discarded errors, budget {budget}")
            for path, number, label in findings[: count - budget + 5]:
                print(f"  {path}:{number}: {label}")
            print(
                f"  Handle {count - budget} of them, or raise the budget in "
                f"{BUDGET_FILE.name} and say why."
            )
        elif count < budget:
            slack.append(f"  {name}: {count} (budget {budget})")

    for name in budgets:
        if not (ROOT / "crates" / name / "src").is_dir():
            print(f"error: {BUDGET_FILE.name} has an entry for missing crate '{name}'")
            failed = True

    if slack:
        print("Below budget — lower these to lock in the improvement:")
        print("\n".join(slack))

    if not failed:
        print("swallowed error budget: ok")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
