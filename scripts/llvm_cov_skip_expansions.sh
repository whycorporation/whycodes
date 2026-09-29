#!/bin/sh
# Inject `-skip-expansions` into `llvm-cov export` only.
#
# cargo-llvm-cov ProcessBuilder strips LLVM_COV_FLAGS (and often other
# WHYCODES_* env) from the child. Bake the real binary into this file
# at generation time (replace the @@REAL@@ placeholder below).
# rustup llvm-cov 21 rejects the flag on `show`; only `export` gets it.
#
# Do not compare `$real` to that placeholder. Substitution rewrites every
# copy, so the guard becomes "path equals the baked path" and, once the
# env override is stripped, the wrapper always exits 1.
set -eu

real="${WHYCODES_LLVM_COV_REAL:-@@REAL@@}"
if [ -z "$real" ] || [ ! -x "$real" ]; then
    printf 'error: llvm-cov wrapper has no real binary path (%s)\n' "$real" >&2
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
