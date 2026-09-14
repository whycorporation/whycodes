#!/bin/sh
# Inject `-skip-expansions` into `llvm-cov export` only.
#
# cargo-llvm-cov ProcessBuilder strips LLVM_COV_FLAGS (and often other
# WHYCODES_* env) from the child. Bake the real binary into this file
# at generation time (`coverage.sh` writes a copy with @@REAL@@ replaced).
# rustup llvm-cov 21 rejects the flag on `show`; only `export` gets it.
set -eu

real="${WHYCODES_LLVM_COV_REAL:-@@REAL@@}"
if [ "$real" = "@@REAL@@" ] || [ -z "$real" ]; then
    printf 'error: llvm-cov wrapper has no real binary path\n' >&2
    exit 1
fi

is_export=0
have_skip=0
for arg in "$@"; do
    case "$arg" in
        export) is_export=1 ;;
        -skip-expansions|--skip-expansions) have_skip=1 ;;
    esac
done

if [ "$is_export" -eq 1 ] && [ "$have_skip" -eq 0 ]; then
    printf 'llvm-cov wrapper: injecting -skip-expansions\n' >&2
    exec "$real" "$@" -skip-expansions
fi

exec "$real" "$@"
