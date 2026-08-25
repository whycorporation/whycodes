#!/usr/bin/env python3
"""Fail when a crate gains a workspace dependency it did not have.

Reads the `whycodes-*` path dependencies out of every crate's Cargo.toml and
compares the graph against `dependency_boundaries.json`.

Adding an edge is what this catches. It is not a claim that the current graph is
correct — it is a claim that the graph should change deliberately, in a commit
that says so, rather than because a file needed one import.

Removing an edge always passes and is reported.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BOUNDARIES = Path(__file__).resolve().parent / "dependency_boundaries.json"

DEP = re.compile(r"^\s*whycodes-([a-z0-9-]+)\s*=", re.MULTILINE)


def edges() -> dict[str, list[str]]:
    """The current graph: crate name -> sorted workspace dependencies."""
    graph: dict[str, list[str]] = {}
    for crate in sorted(p for p in (ROOT / "crates").iterdir() if (p / "Cargo.toml").is_file()):
        text = (crate / "Cargo.toml").read_text(encoding="utf-8")
        # The package's own `name = "whycodes-x"` line is not a dependency.
        deps = {m.group(1) for m in DEP.finditer(text)} - {crate.name}
        graph[crate.name] = sorted(deps)
    return graph


def main() -> int:
    allowed = json.loads(BOUNDARIES.read_text(encoding="utf-8"))
    actual = edges()

    failed = False
    for crate, deps in actual.items():
        permitted = set(allowed.get(crate, []))
        added = sorted(set(deps) - permitted)
        removed = sorted(permitted - set(deps))

        if crate not in allowed:
            print(f"error: crate '{crate}' is not listed in {BOUNDARIES.name}")
            failed = True
            continue
        if added:
            failed = True
            print(f"\n{crate} gained a dependency: {', '.join(added)}")
            print(
                f"  If that is intended, add it to {BOUNDARIES.name} in the same "
                f"commit and say why in the message."
            )
        if removed:
            print(f"{crate} no longer depends on {', '.join(removed)} — "
                  f"remove it from {BOUNDARIES.name} to lock that in.")

    for crate in allowed:
        if crate not in actual:
            print(f"error: {BOUNDARIES.name} lists missing crate '{crate}'")
            failed = True

    if not failed:
        print("dependency boundaries: ok")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
