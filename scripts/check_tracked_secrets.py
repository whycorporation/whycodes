#!/usr/bin/env python3
"""Fail when the index tracks local scratch, credentials, or live-looking secrets.

This is a gate on *current* `HEAD` / the index, not on rewritten history.
Author emails and already-pushed commits are out of scope.

The path denylist is the high-value check: `.omo/` session JSON and
`.whycode/` coverage dumps were committed before ignore rules existed, and
those blobs still carry machine paths. Catching a re-add is cheap.

Content matches are deliberately narrow (private keys, GitHub PATs, Slack
bot tokens, AWS access key ids). Placeholder strings such as `sk-ant-...`
and public installed-app OAuth client secrets are not secrets.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path


def repo_root() -> Path:
    proc = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        check=True,
        capture_output=True,
        text=True,
    )
    return Path(proc.stdout.strip())


FORBIDDEN_DIR_PREFIXES = (
    ".omo/",
    ".whycode/",
    ".whycodes/todos",
    ".whycodes/agents",
    ".whycodes/swarm",
    ".cline/",
    ".claude/",
    "local/",
    "__pycache__/",
)

FORBIDDEN_SUFFIXES = (
    ".pem",
    ".p12",
    ".pfx",
    ".key",
    ".pyc",
    ".profraw",
    ".profdata",
)

FORBIDDEN_BASENAMES = {
    ".env",
    "auth.json",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "lcov.info",
    "cobertura.xml",
    "tarpaulin-report.html",
}

# Live-looking credentials only. Placeholders and public OAuth client ids
# (Gemini CLI / Antigravity installed-app secrets) are documented as public.
CONTENT_PATTERNS: list[tuple[re.Pattern[str], str]] = [
    (re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----"), "PEM private key"),
    (re.compile(r"\bghp_[A-Za-z0-9]{36}\b"), "GitHub PAT (ghp_)"),
    (re.compile(r"\bgithub_pat_[A-Za-z0-9_]{20,}\b"), "GitHub fine-grained PAT"),
    (re.compile(r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b"), "Slack token"),
    (re.compile(r"\bAKIA[0-9A-Z]{16}\b"), "AWS access key id"),
]

MAX_SCAN_BYTES = 1_000_000


def git_ls_files(root: Path) -> list[str]:
    proc = subprocess.run(
        ["git", "ls-files", "-z"],
        cwd=root,
        check=True,
        capture_output=True,
    )
    if not proc.stdout:
        return []
    return [p.decode("utf-8", "surrogateescape") for p in proc.stdout.split(b"\0") if p]


def path_reason(path: str) -> str | None:
    name = path.rsplit("/", 1)[-1]
    if name in FORBIDDEN_BASENAMES:
        return f"forbidden basename {name}"
    if name.startswith(".env.") and name not in {".env.example", ".env.sample"}:
        return "env file"
    for prefix in FORBIDDEN_DIR_PREFIXES:
        stem = prefix.rstrip("/")
        if path == stem or path.startswith(stem + "/"):
            return f"ignored scratch directory {stem}/"
    lower = path.lower()
    for suffix in FORBIDDEN_SUFFIXES:
        if lower.endswith(suffix):
            return f"forbidden suffix {suffix}"
    return None


def scan_content(path: str, data: bytes) -> list[str]:
    if len(data) > MAX_SCAN_BYTES:
        return []
    if b"\0" in data[:4096]:
        return []
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        text = data.decode("utf-8", "replace")
    hits: list[str] = []
    for pattern, label in CONTENT_PATTERNS:
        if pattern.search(text):
            hits.append(label)
    return hits


def main() -> int:
    try:
        root = repo_root()
        files = git_ls_files(root)
    except (OSError, subprocess.CalledProcessError) as exc:
        print(f"check_tracked_secrets: git ls-files failed: {exc}", file=sys.stderr)
        return 2

    failures: list[str] = []
    for path in files:
        reason = path_reason(path)
        if reason:
            failures.append(f"{path}: {reason}")
            continue
        full = root / path
        try:
            data = full.read_bytes()
        except OSError:
            continue
        for label in scan_content(path, data):
            failures.append(f"{path}: live-looking secret ({label})")

    if failures:
        print("check_tracked_secrets: tracked files look like secrets or local scratch:")
        for line in failures:
            print(f"  {line}")
        print(
            "Remove them from the index (git rm --cached) and keep the "
            "gitignore rule. History rewrite is out of scope."
        )
        return 1

    print(f"check_tracked_secrets: ok ({len(files)} tracked files)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
