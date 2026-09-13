#!/usr/bin/env sh
# Inject `-skip-expansions` into `llvm-cov export` only.
#
# cargo-llvm-cov reads LLVM_COV_FLAGS in the parent, then ProcessBuilder
# strips that env from the child. rustup llvm-cov 21 also rejects the flag
# on `show` (text report). Point LLVM_COV at this wrapper for the JSON
# floor export so serde/`format!` lines do not inflate 100% crates.
set -eu

real="${WHYCODES_LLVM_COV_REAL:?WHYCODES_LLVM_COV_REAL must be the real llvm-cov}"

if [ "${1-}" = "export" ]; then
    skip=1
    for arg in "$@"; do
        case "$arg" in
            -skip-expansions|--skip-expansions)
                skip=0
                break
                ;;
        esac
    done
    if [ "$skip" -eq 1 ]; then
        exec "$real" "$@" -skip-expansions
    fi
fi

exec "$real" "$@"
