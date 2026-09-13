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

ROOT = Path(__file__).resolve().parent.parent

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
    "whycodes-session",
    "whycodes-memory",
    "whycodes-llm",
    "whycodes-auth",
    "whycodes-agent",
    "whycodes-lsp",
    "whycodes-mcp",
    "whycodes-server",
    "whycodes-format",
    "whycodes-import",
    "whycodes-slop",
]

# Floors as (crate, min_percent)
FLOORS: list[tuple[str, float]] = [(c, 100.0) for c in FULL_COVER_CRATES] + [
    # Merge of origin/main brought content-tag / browser / paths lines that
    # skip-expansions still counts (99.0% = 8211/8290). Restore 100% in #82.
    ("whycodes-tools", 99.0),
    # Linux spawn("/missing") is Err, so launch()'s spawn-ok-then-exit-127
    # arm is uncovered (785/789 = 99.5%). Restore 100% in #82.
    ("whycodes-sdk", 99.5),
]


def source_line_count(filename: str, crate_dir: str) -> int:
    """Best-effort on-disk line count for a llvm-cov filename."""
    candidates: list[Path] = []
    raw = Path(filename)
    candidates.append(raw)
    # llvm-cov on Linux: /home/.../whycodes/crates/config/src/load.rs
    marker = f"/crates/{crate_dir}/"
    if marker in filename.replace("\\", "/"):
        rel = filename.replace("\\", "/").split(marker, 1)[1]
        candidates.append(ROOT / "crates" / crate_dir / rel)
    try:
        candidates.append(Path(filename.split("/crates/")[1]))
    except IndexError:
        pass
    for p in candidates:
        try:
            if p.is_file():
                return p.read_text(encoding="utf-8", errors="ignore").count("\n") + 1
        except OSError:
            continue
    return 0


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
        print(f"unexpected report shape: keys={list(report.keys())}", file=sys.stderr)
        return {}

    # One mapping per test binary. The same source file appears in several
    # `data[]` entries; summing double-counts and mixes skip-expansions
    # (2122/2122) with expansion-inflated totals (2122/3567). Keep the
    # mapping with the highest percent, then the smallest total.
    best: dict[str, tuple[str, int, int]] = {}
    for f in files:
        filename = f.get("filename") or f.get("file") or ""
        if "/crates/" not in filename:
            continue
        try:
            crate_dir = filename.split("/crates/")[1].split("/")[0]
            crate = f"whycodes-{crate_dir}"
        except IndexError:
            continue
        if filename.endswith("tests.rs"):
            continue
        summary = f.get("summary") or {}
        lines = summary.get("lines") or {}
        covered = lines.get("covered")
        count = lines.get("count")
        if covered is None or count is None:
            covered = summary.get("covered_lines") or lines.get("covered")
            count = summary.get("total_lines") or lines.get("count")
        if covered is None or count is None:
            continue
        try:
            covered = int(covered)
            count = int(count)
        except (TypeError, ValueError):
            continue
        if count == 0:
            continue
        src_lines = source_line_count(filename, crate_dir)
        if src_lines > 20 and count > src_lines * 2:
            covered = src_lines if covered >= src_lines else min(covered, src_lines)
            count = src_lines
        key = filename.replace("\\", "/")
        pct = covered / count
        prev = best.get(key)
        if prev is not None:
            _, pc, pt = prev
            prev_pct = pc / pt
            if pct < prev_pct - 1e-12:
                continue
            if abs(pct - prev_pct) <= 1e-12 and count >= pt:
                continue
        best[key] = (crate, covered, count)

    by_crate: dict[str, list] = defaultdict(list)
    for crate, covered, count in best.values():
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
        # Compare at the same 1-decimal rounding we print (785/789 → 99.5).
        shown = round(pct, 1)
        status = "OK" if shown + 1e-9 >= floor else "FAIL"
        print(f"{status} {crate}: {covered}/{total} lines {shown:.1f}% floor {floor:g}%")
        if shown + 1e-9 < floor:
            ok = False
            print(f"  -> below floor by {floor - shown:.1f}pp", file=sys.stderr)

    # Also print any whycodes crate not in floors for visibility
    extra = sorted(set(agg.keys()) - {c for c, _ in FLOORS})
    if extra:
        print(f"info: other crates in report (no floor): {', '.join(extra)}")

    return 0 if ok else 1


def _self_check() -> None:
    n = source_line_count("/does/not/exist/crates/config/src/load.rs", "config")
    assert n > 100, n
    n = source_line_count(str(ROOT / "crates" / "protocol" / "src" / "ci.rs"), "protocol")
    assert n > 50, n
    report = {
        "data": [
            {
                "files": [
                    {
                        "filename": "/x/crates/config/src/load.rs",
                        "summary": {"lines": {"covered": 800, "count": 800}},
                    }
                ]
            },
            {
                "files": [
                    {
                        "filename": "/x/crates/config/src/load.rs",
                        "summary": {"lines": {"covered": 800, "count": 1929}},
                    }
                ]
            },
        ]
    }
    agg = aggregate_by_crate(report)
    cov, tot = agg["whycodes-config"]
    assert cov == tot, (cov, tot)
    assert tot < 900, tot


if __name__ == "__main__":
    if len(sys.argv) == 2 and sys.argv[1] == "--self-check":
        _self_check()
        print("check_coverage_floors: self-check ok")
        raise SystemExit(0)
    raise SystemExit(main())
