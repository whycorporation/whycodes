#!/usr/bin/env python3
"""Fail when an in-memory cache or retained buffer is allowed to grow.

Every bound listed in `memory_budget.json` is a Rust `const` in a named file:
the most entries a cache holds, the most bytes a buffer keeps, the most
sessions or jobs alive at once. This script reads each constant's current
value from the source and compares it with the recorded one.

Same ratchet as the panic budget. A value **below** the recorded one passes and
is reported, so lowering the JSON is a routine follow-up. A value **above** it
fails, and so does a listed constant that is gone or no longer a plain number:
the only way past is to edit the JSON in the same change, where review sees it.

A new cache is not found automatically. Add its bound to the JSON when you add
the cache (see docs/budgets.md, "Memory").
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BUDGET_NAME = "memory_budget.json"

VALUE = re.compile(r"^\d+(?:\s*\*\s*\d+)*$")


def const_pattern(name: str) -> re.Pattern[str]:
    return re.compile(
        rf"^\s*(?:pub(?:\([a-z]+\))?\s+)?const\s+{re.escape(name)}\s*:\s*[A-Za-z0-9_]+\s*=\s*([^;]+);"
    )


def evaluate(expr: str) -> int | None:
    """`64 * 1024` / `80_000` -> int; anything else (another const, a call) -> None."""
    text = expr.replace("_", "").strip()
    if not VALUE.match(text):
        return None
    value = 1
    for part in text.split("*"):
        value *= int(part)
    return value


def read_value(root: Path, file: str, name: str) -> tuple[int | None, str]:
    path = root / file
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeDecodeError):
        return None, f"{file}: file not found"
    pattern = const_pattern(name)
    hits = [(n, m.group(1)) for n, line in enumerate(lines, 1) if (m := pattern.match(line))]
    if not hits:
        return None, f"{file}: const {name} not found"
    if len(hits) > 1:
        where = ", ".join(str(n) for n, _ in hits)
        return None, f"{file}: const {name} defined more than once (lines {where})"
    number, expr = hits[0]
    value = evaluate(expr)
    if value is None:
        return None, f"{file}:{number}: {name} = {expr.strip()} is not a plain number"
    return value, f"{file}:{number}"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", type=Path, default=ROOT, help="repository root")
    args = parser.parse_args(argv)
    root: Path = args.root
    budget_file = root / "scripts" / BUDGET_NAME

    data = json.loads(budget_file.read_text(encoding="utf-8"))
    entries = data["bounds"]

    failed = False
    slack: list[str] = []
    seen: set[tuple[str, str]] = set()

    for entry in entries:
        file, name, budget = entry["file"], entry["const"], entry["value"]
        if (file, name) in seen:
            print(f"error: {BUDGET_NAME} lists {name} in {file} twice")
            failed = True
            continue
        seen.add((file, name))
        value, where = read_value(root, file, name)
        if value is None:
            failed = True
            print(f"\n{name}: {where}")
            print(
                f"  A budgeted bound disappeared. If the cache went with it, drop the "
                f"entry from {BUDGET_NAME}; if it moved, update `file` and say why."
            )
            continue
        if value > budget:
            failed = True
            print(f"\n{where}: {name} = {value}, budget {budget}")
            print(
                f"  The bound grew. Lower it back, or raise `value` in {BUDGET_NAME} "
                f"in this change and say in the commit message why it needs the memory."
            )
        elif value < budget:
            slack.append(f"  {where}: {name} = {value} (budget {budget})")

    if slack:
        print("Below budget — lower these to lock in the improvement:")
        print("\n".join(slack))

    if not failed:
        print(f"memory budget: ok ({len(entries)} bounds)")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
