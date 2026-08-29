#!/usr/bin/env python3
"""Offline checks for scripts/release_downloads.py."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "release_downloads.py"


def run(*args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=check,
    )


def test_self_test() -> None:
    proc = run("--self-test")
    if "ok" not in proc.stdout:
        raise SystemExit(f"expected ok, got:\n{proc.stdout}{proc.stderr}")


def test_fixture_json() -> None:
    fixture = [
        {
            "tag_name": "v9.9.9",
            "draft": False,
            "prerelease": False,
            "published_at": "2026-08-29T00:00:00Z",
            "assets": [
                {
                    "name": "whycodes-x86_64-pc-windows-msvc.zip",
                    "download_count": 2,
                },
                {"name": "SHA256SUMS", "download_count": 2},
            ],
        }
    ]
    with tempfile.NamedTemporaryFile(
        "w", suffix=".json", delete=False, encoding="utf-8"
    ) as handle:
        json.dump(fixture, handle)
        path = handle.name
    try:
        proc = run("--fixture", path, "--json")
        data = json.loads(proc.stdout)
    finally:
        Path(path).unlink(missing_ok=True)
    if data["binary_downloads"] != 2:
        raise SystemExit(f"binary_downloads: {data}")
    if data["checksum_downloads"] != 2:
        raise SystemExit(f"checksum_downloads: {data}")
    if data["tags"][0]["tag"] != "v9.9.9":
        raise SystemExit(f"tag: {data}")


def test_drafts_skipped() -> None:
    fixture = [
        {
            "tag_name": "v0.0.1",
            "draft": True,
            "assets": [
                {
                    "name": "whycodes-x86_64-unknown-linux-gnu.tar.gz",
                    "download_count": 50,
                }
            ],
        }
    ]
    with tempfile.NamedTemporaryFile(
        "w", suffix=".json", delete=False, encoding="utf-8"
    ) as handle:
        json.dump({"releases": fixture}, handle)
        path = handle.name
    try:
        proc = run("--fixture", path, "--json")
        data = json.loads(proc.stdout)
    finally:
        Path(path).unlink(missing_ok=True)
    if data["releases"] != 0 or data["binary_downloads"] != 0:
        raise SystemExit(f"drafts should be ignored: {data}")


def main() -> int:
    test_self_test()
    test_fixture_json()
    test_drafts_skipped()
    print("test_release_downloads: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
