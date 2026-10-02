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
import os
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
    "whycodes-sdk",
    "whycodes-tools",
]

# Workspace floor. rustup llvm-cov `show` inflates totals with serde /
# format! expansions (~81.4%). JSON `export -skip-expansions` is the
# same measurement as the 100% crate floors.
WORKSPACE_FLOOR = float(os.environ.get("FAIL_UNDER", "82"))

# Floors as (crate, min_percent). `whycodes-tui` and `whycodes-cli` stay
# off this list until a Linux skip-expansions run prints covered == total
# for them. Issue #82.
FLOORS: list[tuple[str, float]] = [(c, 100.0) for c in FULL_COVER_CRATES]

# Extra crates whose uncovered files print even when they meet the floor.
# Empty once a crate is on FULL_COVER_CRATES: a miss already fails the run.
GAP_REPORT: set[str] = set()
GAP_FILE_LIMIT = 25


def crate_rel_path(filename: str) -> tuple[str, str] | None:
    """Return (crate_dir, path-under-crate) from an llvm-cov filename.

    llvm-cov export sometimes uses an absolute path
    (`/home/.../whycodes/crates/session/src/session.rs`) and sometimes a
    workspace-relative one (`crates/session/src/session.rs`). The relative
    form has no leading slash, so a `/crates/` needle misses it and the
    skip-expansions mapping is dropped — only the expansion dump remains.
    """
    norm = filename.replace("\\", "/").lstrip("./")
    rest = None
    if "/crates/" in norm:
        rest = norm.split("/crates/", 1)[1]
    elif norm.startswith("crates/"):
        rest = norm[len("crates/") :]
    if not rest:
        return None
    crate_dir, _, rel = rest.partition("/")
    if not crate_dir or not rel:
        return None
    return crate_dir, rel


def source_line_count(filename: str, crate_dir: str) -> int:
    """Best-effort on-disk line count for a llvm-cov filename."""
    candidates: list[Path] = []
    raw = Path(filename)
    candidates.append(raw)
    parsed = crate_rel_path(filename)
    if parsed is not None:
        dir_name, rel = parsed
        candidates.append(ROOT / "crates" / dir_name / rel)
        if dir_name != crate_dir:
            candidates.append(ROOT / "crates" / crate_dir / rel)
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
    # `data[]` entries (and under different absolute prefixes). Summing
    # double-counts and mixes skip-expansions (2122/2122) with expansion
    # dumps (2122/3567). Keep the mapping with the highest percent, then
    # the smallest total. Key by crates/<dir>/<rel> so prefixes collapse.
    best: dict[str, tuple[str, int, int]] = {}
    for f in files:
        filename = f.get("filename") or f.get("file") or ""
        parsed = crate_rel_path(filename)
        if parsed is None:
            continue
        crate_dir, rel = parsed
        crate = f"whycodes-{crate_dir}"
        if rel.endswith("tests.rs") or filename.endswith("tests.rs"):
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
        # Expansion dumps are ~1.3–2× source (session.rs 3553 vs 4861).
        # Drop them. Do not rewrite covered/count — that turns 2736/4861
        # into a fake 2736/3553 (77%) and sinks the 100% floor.
        if src_lines > 20 and count > src_lines + max(80, src_lines // 8):
            continue
        key = f"{crate_dir}/{rel}"
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
    files_by_crate: dict[str, list] = defaultdict(list)
    for key, (crate, covered, count) in best.items():
        by_crate[crate].append((covered, count))
        files_by_crate[crate].append((key, covered, count))

    aggregated: dict[str, tuple[int, int]] = {}
    for crate, pairs in by_crate.items():
        cov = sum(c for c, _ in pairs)
        tot = sum(t for _, t in pairs)
        aggregated[crate] = (cov, tot)
    aggregate_by_crate.files = files_by_crate  # type: ignore[attr-defined]
    return aggregated


def uncovered_files(crate: str) -> list[tuple[str, int, int]]:
    """Production files in `crate` that still have at least one missed line."""
    files = getattr(aggregate_by_crate, "files", {}).get(crate, [])
    gaps = [(name, cov, tot) for name, cov, tot in files if tot and cov < tot]
    gaps.sort(key=lambda row: (row[2] - row[1], row[0]), reverse=True)
    return gaps


def print_gaps(crate: str, limit: int = GAP_FILE_LIMIT) -> None:
    gaps = uncovered_files(crate)
    shown = gaps[:limit]
    for name, cov, tot in shown:
        fpct = cov / tot * 100.0
        print(f"     {name}: {cov}/{tot} ({fpct:.1f}%) miss {tot - cov}")
    rest = len(gaps) - len(shown)
    if rest:
        print(f"     … {rest} more files with missed lines")


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
    ws_cov = sum(c for c, _ in agg.values())
    ws_tot = sum(t for _, t in agg.values())
    if ws_tot:
        ws_pct = ws_cov / ws_tot * 100.0
        ws_shown = round(ws_pct, 1)
        ws_status = "OK" if ws_shown + 1e-9 >= WORKSPACE_FLOOR else "FAIL"
        print(
            f"{ws_status} workspace: {ws_cov}/{ws_tot} lines "
            f"{ws_shown:.1f}% floor {WORKSPACE_FLOOR:g}%"
        )
        if ws_shown + 1e-9 < WORKSPACE_FLOOR:
            ok = False
            print(
                f"  -> below workspace floor by {WORKSPACE_FLOOR - ws_shown:.1f}pp",
                file=sys.stderr,
            )
    for crate, floor in FLOORS:
        pair = agg.get(crate)
        if pair is None:
            print(f"SKIP {crate}: no files in report (crate may be empty or filtered)", file=sys.stderr)
            continue
        covered, total = pair
        pct = (covered / total * 100.0) if total else 0.0
        # Compare at the same 1-decimal rounding we print.
        shown = round(pct, 1)
        status = "OK" if shown + 1e-9 >= floor else "FAIL"
        print(f"{status} {crate}: {covered}/{total} lines {shown:.1f}% floor {floor:g}%")
        if shown + 1e-9 < floor:
            ok = False
            print(f"  -> below floor by {floor - shown:.1f}pp", file=sys.stderr)
        if shown + 1e-9 < floor or crate in GAP_REPORT:
            print_gaps(crate)

    # Unfloored crates (#82: tui, cli) need a percent in the log. Names
    # alone hid the gap between the 82% workspace gate and a 100% close.
    extra = sorted(set(agg.keys()) - {c for c, _ in FLOORS})
    for crate in extra:
        covered, total = agg[crate]
        pct = (covered / total * 100.0) if total else 0.0
        print(f"info {crate}: {covered}/{total} lines {pct:.1f}% (no floor)")
        print_gaps(crate)

    return 0 if ok else 1


def _self_check() -> None:
    n = source_line_count("/does/not/exist/crates/config/src/load.rs", "config")
    assert n > 100, n
    n = source_line_count(str(ROOT / "crates" / "protocol" / "src" / "ci.rs"), "protocol")
    assert n > 50, n
    assert crate_rel_path("crates/session/src/session.rs") == (
        "session",
        "src/session.rs",
    )
    assert crate_rel_path("./crates/llm/src/openai_compat.rs") == (
        "llm",
        "src/openai_compat.rs",
    )
    report = {
        "data": [
            {
                "files": [
                    {
                        "filename": "crates/session/src/session.rs",
                        "summary": {"lines": {"covered": 3500, "count": 3500}},
                    },
                    {
                        "filename": "/runner-a/crates/config/src/load.rs",
                        "summary": {"lines": {"covered": 800, "count": 800}},
                    },
                ]
            },
            {
                "files": [
                    {
                        "filename": "/runner-b/crates/session/src/session.rs",
                        "summary": {"lines": {"covered": 2736, "count": 4861}},
                    },
                    {
                        "filename": "/runner-b/crates/config/src/load.rs",
                        "summary": {"lines": {"covered": 800, "count": 1929}},
                    },
                ]
            },
        ]
    }
    agg = aggregate_by_crate(report)
    cov, tot = agg["whycodes-config"]
    assert cov == tot, (cov, tot)
    assert tot < 900, tot
    scov, stot = agg["whycodes-session"]
    assert scov == stot == 3500, (scov, stot)
    files = getattr(aggregate_by_crate, "files", {})
    assert "whycodes-config" in files
    assert files["whycodes-config"][0][0] == "config/src/load.rs"
    assert WORKSPACE_FLOOR >= 0
    gap_report = {
        "data": [
            {
                "files": [
                    {
                        "filename": "crates/cli/src/main.rs",
                        "summary": {"lines": {"covered": 40, "count": 100}},
                    },
                    {
                        "filename": "crates/tools/src/web/browser.rs",
                        "summary": {"lines": {"covered": 90, "count": 100}},
                    },
                    {
                        "filename": "crates/tools/src/lib.rs",
                        "summary": {"lines": {"covered": 10, "count": 10}},
                    },
                ]
            }
        ]
    }
    aggregate_by_crate(gap_report)
    tools_gaps = uncovered_files("whycodes-tools")
    assert tools_gaps == [("tools/src/web/browser.rs", 90, 100)], tools_gaps
    assert uncovered_files("whycodes-cli") == [("cli/src/main.rs", 40, 100)]
    assert "whycodes-tools" not in GAP_REPORT
    assert "whycodes-tools" in FULL_COVER_CRATES


if __name__ == "__main__":
    if len(sys.argv) == 2 and sys.argv[1] == "--self-check":
        _self_check()
        print("check_coverage_floors: self-check ok")
        raise SystemExit(0)
    raise SystemExit(main())
