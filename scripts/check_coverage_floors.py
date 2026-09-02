#!/usr/bin/env python3
"""Check per-crate line coverage floors from a single cargo llvm-cov JSON report.

Replaces the previous 12 separate `cargo llvm-cov -p <crate>` invocations.
Single workspace instrumentation run (~90s) + this script (<1s) instead of
12× re-instrumentation (~240s) on the single self-hosted runner.

Usage:
  scripts/coverage.sh
  # or, same steps by hand:
  cargo llvm-cov --workspace --ignore-filename-regex "$IGNORE" --summary-only -- --skip ...
  cargo llvm-cov report --json --ignore-filename-regex "$CRATE_IGNORE" --summary-only > /tmp/cov.json
  python3 scripts/check_coverage_floors.py /tmp/cov.json

Expects JSON from `cargo llvm-cov report --json --summary-only` or
`cargo llvm-cov --json --summary-only`. Falls back to parsing `data[].files[]`.
"""

from __future__ import annotations

import json
import sys
from collections import defaultdict
from pathlib import Path

# Crates that must be 100% line-covered (ignore *tests.rs so host-only branches don't sink floor)
FULL_COVER_CRATES = [
    "whycodes-function",
    "whycodes-schema",
    "whycodes-skill",
    "whycodes-sandbox",
    "whycodes-protocol",
    "whycodes-plugin",
    "whycodes-command-risk",
    "whycodes-storage",
    "whycodes-core",
    "whycodes-config",
    "whycodes-index",
]

# Floors as (crate, min_percent)
FLOORS: list[tuple[str, float]] = [(c, 100.0) for c in FULL_COVER_CRATES] + [
    ("whycodes-format", 95.0),
]


def load_report(path: Path) -> dict:
    raw = json.loads(path.read_text())
    # cargo-llvm-cov json nests under `data` (llvm-cov export) or top-level `files`
    if isinstance(raw, dict) and "data" in raw:
        return raw
    return raw


def aggregate_by_crate(report: dict) -> dict[str, tuple[int, int]]:
    """Return {crate: (covered_lines, total_lines)}."""
    # llvm-cov export JSON: data[].files[].summary.lines.{count,covered,percent}
    # or files[].summary
    files = []
    if "data" in report:
        for entry in report["data"]:
            files.extend(entry.get("files", []))
    elif "files" in report:
        files = report["files"]
    else:
        # unexpected shape — treat as empty to fail loudly
        print(f"unexpected report shape: keys={list(report.keys())}", file=sys.stderr)
        return {}

    by_crate: dict[str, list] = defaultdict(list)
    for f in files:
        filename = f.get("filename") or f.get("file") or ""
        # Only consider our crate files under crates/<name>/
        # filename is absolute or relative; look for /crates/<crate>/
        if "/crates/" not in filename:
            continue
        # Extract crate dir name after /crates/
        try:
            crate_dir = filename.split("/crates/")[1].split("/")[0]
            crate = f"whycodes-{crate_dir}"
        except IndexError:
            continue
        # Skip tests.rs basenames when caller already filtered, but double-guard
        if filename.endswith("tests.rs"):
            continue
        summary = f.get("summary") or {}
        lines = summary.get("lines") or {}
        covered = lines.get("covered")
        count = lines.get("count")
        # Some versions use `covered`/`count`, others `covered`/`total`
        if covered is None or count is None:
            # Try alternative keys
            covered = summary.get("covered_lines") or lines.get("covered")
            count = summary.get("total_lines") or lines.get("count")
        if covered is None or count is None:
            continue
        # Normalize
        try:
            covered = int(covered)
            count = int(count)
        except (TypeError, ValueError):
            continue
        if count == 0:
            continue
        by_crate[crate].append((covered, count))

    aggregated: dict[str, tuple[int, int]] = {}
    for crate, pairs in by_crate.items():
        cov = sum(c for c, _ in pairs)
        tot = sum(t for _, t in pairs)
        aggregated[crate] = (cov, tot)
    return aggregated


def main() -> int:
    if len(sys.argv) < 2:
        print(f"usage: {sys.argv[0]} <cov.json>", file=sys.stderr)
        return 2
    report_path = Path(sys.argv[1])
    if not report_path.exists():
        print(f"report not found: {report_path}", file=sys.stderr)
        return 2

    report = load_report(report_path)
    agg = aggregate_by_crate(report)

    if not agg:
        print("no crate data found in report — did you run with --summary-only --json ?", file=sys.stderr)
        # Dump a hint of the report keys
        print(f"report keys: {list(report.keys())}", file=sys.stderr)
        return 1

    ok = True
    for crate, floor in FLOORS:
        pair = agg.get(crate)
        if pair is None:
            print(f"SKIP {crate}: no files in report (crate may be empty or filtered)", file=sys.stderr)
            continue
        covered, total = pair
        pct = (covered / total * 100.0) if total else 0.0
        status = "OK" if pct + 1e-9 >= floor else "FAIL"
        print(f"{status} {crate}: {covered}/{total} lines {pct:.1f}% floor {floor:.0f}%")
        if pct + 1e-9 < floor:
            ok = False
            # Show missing lines hint if available
            print(f"  -> below floor by {floor - pct:.1f}pp", file=sys.stderr)

    # Also print any whycodes crate not in floors for visibility
    extra = sorted(set(agg.keys()) - {c for c, _ in FLOORS})
    if extra:
        print(f"info: other crates in report (no floor): {', '.join(extra)}")

    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
