#!/usr/bin/env python3
"""Print GitHub release asset download counts.

This is the install-side statistic: how often published binaries were
fetched (`install.sh`, Homebrew, `whycodes upgrade`, a browser click).
It is not unique people, daily active use, or `cargo install`. The CLI
does not phone home; this script only talks to the GitHub API when you
run it.

  python scripts/release_downloads.py
  python scripts/release_downloads.py --json
  python scripts/release_downloads.py --repo owner/name
  python scripts/release_downloads.py --fixture path.json
  python scripts/release_downloads.py --self-test
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

DEFAULT_REPO = "whycorporation/whycodes"
API_ACCEPT = "application/vnd.github+json"
USER_AGENT = "whycodes-release-downloads"

BINARY_SUFFIXES = (".tar.gz", ".zip")


def github_token() -> str | None:
    for key in ("GITHUB_TOKEN", "GH_TOKEN"):
        value = os.environ.get(key, "").strip()
        if value:
            return value
    return None


def classify_asset(name: str) -> str:
    """Bucket a release asset. Checksums are not unique installs."""
    if name == "SHA256SUMS" or name.endswith(".sha256"):
        return "checksum"
    lower = name.lower()
    if lower.endswith(BINARY_SUFFIXES):
        return "binary"
    return "other"


def summarize(releases: list[dict[str, Any]]) -> dict[str, Any]:
    """Fold GitHub `/releases` JSON into per-tag and headline totals."""
    tags: list[dict[str, Any]] = []
    binary_total = 0
    checksum_total = 0
    other_total = 0
    by_asset: dict[str, int] = {}

    for rel in releases:
        if rel.get("draft"):
            continue
        tag = rel.get("tag_name") or rel.get("name") or "?"
        assets_out: list[dict[str, Any]] = []
        tag_binary = 0
        for asset in rel.get("assets") or []:
            name = asset.get("name") or ""
            count = int(asset.get("download_count") or 0)
            kind = classify_asset(name)
            assets_out.append({"name": name, "kind": kind, "downloads": count})
            by_asset[name] = by_asset.get(name, 0) + count
            if kind == "binary":
                tag_binary += count
                binary_total += count
            elif kind == "checksum":
                checksum_total += count
            else:
                other_total += count
        assets_out.sort(key=lambda a: (-a["downloads"], a["name"]))
        tags.append(
            {
                "tag": tag,
                "prerelease": bool(rel.get("prerelease")),
                "published_at": rel.get("published_at"),
                "binary_downloads": tag_binary,
                "assets": assets_out,
            }
        )

    tags.sort(key=lambda t: t.get("published_at") or "", reverse=True)
    return {
        "binary_downloads": binary_total,
        "checksum_downloads": checksum_total,
        "other_downloads": other_total,
        "releases": len(tags),
        "by_asset": dict(sorted(by_asset.items(), key=lambda kv: (-kv[1], kv[0]))),
        "tags": tags,
        "note": (
            "binary_downloads counts archive fetches, not unique people. "
            "Upgrades, CI, and retries inflate it; cargo install is invisible. "
            "SHA256SUMS is listed separately because install.sh downloads it "
            "alongside each archive."
        ),
    }


def format_table(summary: dict[str, Any]) -> str:
    lines = [
        f"binary archive downloads: {summary['binary_downloads']}",
        f"SHA256SUMS downloads:     {summary['checksum_downloads']}",
        f"other assets:             {summary['other_downloads']}",
        f"releases:                 {summary['releases']}",
        "",
        summary["note"],
        "",
    ]
    if not summary["tags"]:
        lines.append("(no published releases)")
        return "\n".join(lines)

    lines.append(f"{'tag':<16} {'binary':>8}  assets")
    for tag in summary["tags"]:
        parts = [
            f"{a['name']} {a['downloads']}"
            for a in tag["assets"]
            if a["kind"] == "binary"
        ]
        extra = f"  {', '.join(parts)}" if parts else ""
        lines.append(f"{tag['tag']:<16} {tag['binary_downloads']:>8}{extra}")
    return "\n".join(lines)


def fetch_releases(repo: str, token: str | None) -> list[dict[str, Any]]:
    url = f"https://api.github.com/repos/{repo}/releases?per_page=100"
    req = urllib.request.Request(
        url,
        headers={
            "Accept": API_ACCEPT,
            "User-Agent": USER_AGENT,
        },
    )
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            payload = json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        body = exc.read().decode("utf-8", "replace")[:300]
        raise SystemExit(
            f"error: GitHub API {exc.code} for {repo}: {body}"
        ) from exc
    except urllib.error.URLError as exc:
        raise SystemExit(f"error: could not reach GitHub API: {exc}") from exc
    if not isinstance(payload, list):
        raise SystemExit("error: GitHub API did not return a release list")
    return payload


def load_fixture(path: str) -> list[dict[str, Any]]:
    data = json.loads(Path(path).read_text(encoding="utf-8"))
    if isinstance(data, dict) and "releases" in data:
        data = data["releases"]
    if not isinstance(data, list):
        raise SystemExit("error: fixture must be a JSON array of releases")
    return data


def self_test() -> int:
    fixture = [
        {
            "tag_name": "v0.2.0",
            "draft": False,
            "prerelease": False,
            "published_at": "2026-08-20T00:00:00Z",
            "assets": [
                {
                    "name": "whycodes-x86_64-unknown-linux-gnu.tar.gz",
                    "download_count": 10,
                },
                {
                    "name": "whycodes-aarch64-apple-darwin.tar.gz",
                    "download_count": 4,
                },
                {"name": "SHA256SUMS", "download_count": 12},
            ],
        },
        {
            "tag_name": "v0.1.0",
            "draft": False,
            "prerelease": False,
            "published_at": "2026-08-01T00:00:00Z",
            "assets": [
                {"name": "whycode-x86_64-unknown-linux-gnu.tar.gz", "download_count": 3},
                {"name": "notes.md", "download_count": 1},
            ],
        },
        {
            "tag_name": "v0.0.0-draft",
            "draft": True,
            "assets": [
                {
                    "name": "whycodes-x86_64-unknown-linux-gnu.tar.gz",
                    "download_count": 99,
                }
            ],
        },
    ]
    summary = summarize(fixture)
    assert summary["releases"] == 2, summary["releases"]
    assert summary["binary_downloads"] == 17, summary["binary_downloads"]
    assert summary["checksum_downloads"] == 12, summary["checksum_downloads"]
    assert summary["other_downloads"] == 1, summary["other_downloads"]
    assert summary["tags"][0]["tag"] == "v0.2.0"
    assert summary["by_asset"]["whycodes-x86_64-unknown-linux-gnu.tar.gz"] == 10
    text = format_table(summary)
    assert "binary archive downloads: 17" in text
    assert "v0.2.0" in text
    assert "v0.0.0-draft" not in text
    print("release_downloads: ok")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--repo",
        default=os.environ.get("WHYCODES_REPO", DEFAULT_REPO),
        help="owner/name (default: whycorporation/whycodes)",
    )
    ap.add_argument(
        "--fixture",
        help="read GitHub-shaped JSON instead of calling the API",
    )
    ap.add_argument("--json", action="store_true", help="print the summary as JSON")
    ap.add_argument(
        "--self-test",
        action="store_true",
        help="run the offline fixture checks and exit",
    )
    args = ap.parse_args()

    if args.self_test:
        return self_test()

    if args.fixture:
        releases = load_fixture(args.fixture)
    else:
        releases = fetch_releases(args.repo, github_token())

    summary = summarize(releases)
    summary["repo"] = args.repo
    if args.json:
        json.dump(summary, sys.stdout, indent=2)
        sys.stdout.write("\n")
    else:
        print(format_table(summary))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
