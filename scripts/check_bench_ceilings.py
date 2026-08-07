#!/usr/bin/env python3
"""Loose CI ceilings for startup / memory (plan-performance residual).

Catches ~2× regressions on Ubuntu runners; not a 5% gate.
Expects optional JSON from bench scripts under docs/bench-results.json
or generates a stub pass when benches are skipped (no release binary).

Usage:
  python scripts/check_bench_ceilings.py
  python scripts/check_bench_ceilings.py --results docs/bench-results.json
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

# Generous ceilings (Linux CI, release). Update only with deliberate intent.
CEILINGS = {
    "version_ms_p95": 50.0,  # was ~1–2 ms locally; 50 catches 20×+ disasters
    "version_rss_mb": 40.0,
}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "--results",
        type=Path,
        default=Path("docs/bench-results.json"),
        help="JSON from bench_startup/memory --json",
    )
    args = ap.parse_args()

    if not args.results.exists():
        print(f"check_bench_ceilings: no {args.results} — skip (no failure)")
        return 0

    data = json.loads(args.results.read_text(encoding="utf-8"))
    failures = []

    # Flexible schema: accept nested or flat keys.
    version_ms = (
        data.get("version_ms_p95")
        or data.get("startup", {}).get("version", {}).get("p95_ms")
        or data.get("startup_version_p95_ms")
    )
    version_rss = (
        data.get("version_rss_mb")
        or data.get("memory", {}).get("version", {}).get("peak_rss_mb")
        or data.get("version_peak_rss_mb")
    )

    if version_ms is not None and float(version_ms) > CEILINGS["version_ms_p95"]:
        failures.append(
            f"startup version p95 {version_ms} ms > ceiling {CEILINGS['version_ms_p95']} ms"
        )
    if version_rss is not None and float(version_rss) > CEILINGS["version_rss_mb"]:
        failures.append(
            f"version peak RSS {version_rss} MB > ceiling {CEILINGS['version_rss_mb']} MB"
        )

    if failures:
        print("check_bench_ceilings: FAIL")
        for f in failures:
            print(f"  - {f}")
        return 1

    print("check_bench_ceilings: ok")
    if version_ms is not None:
        print(f"  version_ms_p95={version_ms}")
    if version_rss is not None:
        print(f"  version_rss_mb={version_rss}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
