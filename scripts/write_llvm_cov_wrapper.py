#!/usr/bin/env python3
"""Write an llvm-cov wrapper that injects -skip-expansions on export.

cargo-llvm-cov 0.9.1 uses LLVM_COV only when LLVM_PROFDATA is set, and
strips child env. Bake the real binary path into a file named llvm-cov.
"""

from __future__ import annotations

import os
import shlex
import stat
import sys
from pathlib import Path


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: write_llvm_cov_wrapper.py <real-llvm-cov> <dest>", file=sys.stderr)
        return 2
    real = sys.argv[1]
    dest = Path(sys.argv[2])
    dest.parent.mkdir(parents=True, exist_ok=True)
    quoted = shlex.quote(real)
    body = f"""#!/bin/sh
set -eu
real={quoted}
is_export=0
have_skip=0
for arg in "$@"; do
    case "$arg" in
        export) is_export=1 ;;
        -skip-expansions|--skip-expansions) have_skip=1 ;;
    esac
done
if [ "$is_export" -eq 1 ] && [ "$have_skip" -eq 0 ]; then
    printf 'llvm-cov wrapper: injecting -skip-expansions\\n' >&2
    exec "$real" "$@" -skip-expansions
fi
exec "$real" "$@"
"""
    dest.write_bytes(body.encode("utf-8"))
    dest.chmod(dest.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
