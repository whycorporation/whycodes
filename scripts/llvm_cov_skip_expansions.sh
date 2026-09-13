#!/usr/bin/env sh
# Inject `-skip-expansions` into `llvm-cov export` only.
#
# cargo-llvm-cov reads LLVM_COV_FLAGS in the parent, then ProcessBuilder
# strips that env from the child. rustup llvm-cov 21 also rejects the flag
# on `show` (text report). Point LLVM_COV at this wrapper for the JSON
# floor export so serde/`format!` lines do not inflate 100% crates.
set -eu

real="${WHYCODES_LLVM_COV_REAL:?WHYCODES_LLVM_COV_REAL must be the real llvm-cov}"

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
