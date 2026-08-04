#!/usr/bin/env python3
"""Fail when a crate gains ways to panic.

Counts `unwrap()`, `expect()`, `panic!()`, `todo!()` and `unimplemented!()` in
non-test code and compares each crate against `panic_budget.json`.

The budget is a ratchet. Going below it passes and is reported, so lowering the
number is a routine follow-up commit. Going above it fails, and the only way to
raise it is to edit the JSON — which shows up in review, which is the point.

Test code is excluded: `unwrap()` in a test is how a test asserts. Files under
`tests/` and blocks under `#[cfg(test)]` are skipped.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BUDGET_FILE = Path(__file__).resolve().parent / "panic_budget.json"

PATTERNS = [
    (re.compile(r"\.unwrap\(\)"), "unwrap()"),
    (re.compile(r"\.expect\("), "expect()"),
    (re.compile(r"\bpanic!\("), "panic!()"),
    (re.compile(r"\btodo!\("), "todo!()"),
    (re.compile(r"\bunimplemented!\("), "unimplemented!()"),
]

CFG_TEST = re.compile(r"^\s*#\[cfg\(test\)\]")


def strip_test_code(lines: list[str]) -> list[tuple[int, str]]:
    """Return (line number, text) for lines outside `#[cfg(test)]` blocks.

    Brace counting rather than parsing: a test module is the block that follows
    the attribute, so track depth from its opening brace to its closing one.

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
            # External module: `mod tests;` — body lives in tests.rs, not here.
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
    """Every panic site in a crate's non-test sources."""
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
                for _ in pattern.finditer(code):
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
            print(f"\n{name}: {count} panic sites, budget {budget}")
            for path, number, label in findings[: count - budget + 5]:
                print(f"  {path}:{number}: {label}")
            print(
                f"  Remove {count - budget} of them, or raise the budget in "
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
        print("panic budget: ok")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
