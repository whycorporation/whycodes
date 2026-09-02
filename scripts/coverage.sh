#!/usr/bin/env sh
# Run the same cargo-llvm-cov measurement as CI `Coverage (line floor)`.
#
# Usage:
#   scripts/coverage.sh                 # workspace summary + per-crate floors
#   scripts/coverage.sh --dry-run       # print commands, do not exec
#   scripts/coverage.sh --help
#   FAIL_UNDER=90 scripts/coverage.sh
#   COVERAGE_FEATURES=whycodes-storage/bundled scripts/coverage.sh
#   REPORT_JSON=/tmp/cov.json scripts/coverage.sh
#
# Needs cargo-llvm-cov and llvm-tools (`llvm-cov`, `llvm-profdata`).
# rustup: rust-toolchain.toml installs llvm-tools-preview.
# Distro rust: export LLVM_COV=$(command -v llvm-cov)
#              LLVM_PROFDATA=$(command -v llvm-profdata)

set -eu

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

IGNORE="${IGNORE:-/usr/src/|/rustc-}"
CRATE_IGNORE="${CRATE_IGNORE:-${IGNORE}|tests\\.rs$}"
FAIL_UNDER="${FAIL_UNDER:-82}"
REPORT_JSON="${REPORT_JSON:-/tmp/cov.json}"
COVERAGE_FEATURES="${COVERAGE_FEATURES:-}"

dry_run=0

say() { printf '%s\n' "$*"; }
err() { printf 'error: %s\n' "$*" >&2; }

usage() {
    sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
    case "$1" in
        --help|-h)
            usage
            exit 0
            ;;
        --dry-run)
            dry_run=1
            shift
            ;;
        *)
            err "unknown argument: $1"
            usage >&2
            exit 2
            ;;
    esac
done

run() {
    if [ "$dry_run" -eq 1 ]; then
        printf '+'
        for arg in "$@"; do
            printf ' %s' "$arg"
        done
        printf '\n'
    else
        "$@"
    fi
}

need_cmd() {
    if ! command -v "$1" >/dev/null 2>&1; then
        err "$1 not found on PATH"
        shift
        if [ $# -gt 0 ]; then
            say "$*" >&2
        fi
        exit 1
    fi
}

# cargo-llvm-cov looks up rustup's llvm-tools first. Distro rustc has no
# rustup; point at system binaries when the user has not already set them.
if [ -z "${LLVM_COV:-}" ] && command -v llvm-cov >/dev/null 2>&1; then
    LLVM_COV="$(command -v llvm-cov)"
    export LLVM_COV
fi
if [ -z "${LLVM_PROFDATA:-}" ] && command -v llvm-profdata >/dev/null 2>&1; then
    LLVM_PROFDATA="$(command -v llvm-profdata)"
    export LLVM_PROFDATA
fi

cov_cmd="cargo llvm-cov --workspace"
if [ -n "$COVERAGE_FEATURES" ]; then
    cov_cmd="$cov_cmd --features $COVERAGE_FEATURES"
fi
cov_cmd="$cov_cmd --ignore-filename-regex $IGNORE --fail-under-lines $FAIL_UNDER --summary-only -- --skip tests::watcher_picks_up_changes --skip picker_flow_over_real_index"

report_cmd="cargo llvm-cov report --json --ignore-filename-regex $CRATE_IGNORE --summary-only"
floors_cmd="python3 scripts/check_coverage_floors.py $REPORT_JSON"

if [ "$dry_run" -eq 1 ]; then
    say "+ $cov_cmd"
    say "+ $report_cmd > $REPORT_JSON"
    say "+ $floors_cmd"
    exit 0
fi

need_cmd cargo "install the Rust toolchain (rustup or distro rust)"
need_cmd python3
if ! command -v cargo-llvm-cov >/dev/null 2>&1 && ! cargo llvm-cov --version >/dev/null 2>&1; then
    err "cargo-llvm-cov not found"
    say "install: cargo install cargo-llvm-cov --locked" >&2
    exit 1
fi
if [ -z "${LLVM_COV:-}" ] || ! command -v "$LLVM_COV" >/dev/null 2>&1; then
    if ! command -v llvm-cov >/dev/null 2>&1; then
        err "llvm-cov not found"
        say "rustup: rustup component add llvm-tools-preview" >&2
        say "distro: export LLVM_COV=\$(command -v llvm-cov) LLVM_PROFDATA=\$(command -v llvm-profdata)" >&2
        exit 1
    fi
fi

# Rebuild argv without going through the shell so regex metacharacters stay literal.
set -- cargo llvm-cov --workspace
if [ -n "$COVERAGE_FEATURES" ]; then
    set -- "$@" --features "$COVERAGE_FEATURES"
fi
set -- "$@" \
    --ignore-filename-regex "$IGNORE" \
    --fail-under-lines "$FAIL_UNDER" \
    --summary-only \
    -- \
    --skip tests::watcher_picks_up_changes \
    --skip picker_flow_over_real_index
run "$@"

run cargo llvm-cov report --json --ignore-filename-regex "$CRATE_IGNORE" --summary-only >"$REPORT_JSON"
run python3 scripts/check_coverage_floors.py "$REPORT_JSON"
