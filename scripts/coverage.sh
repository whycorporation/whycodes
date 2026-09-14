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

# GITHUB_ENV from a CRLF script can leave CR on path vars; cargo then
# looks at a different directory than the one we stamp/clean.
strip_cr() {
    printf '%s' "$1" | tr -d '\r'
}
CARGO_TARGET_DIR="$(strip_cr "${CARGO_TARGET_DIR:-}")"
CARGO_LLVM_COV_TARGET_DIR="$(strip_cr "${CARGO_LLVM_COV_TARGET_DIR:-}")"
CARGO_HOME="$(strip_cr "${CARGO_HOME:-}")"
[ -n "$CARGO_TARGET_DIR" ] && export CARGO_TARGET_DIR
[ -n "$CARGO_LLVM_COV_TARGET_DIR" ] && export CARGO_LLVM_COV_TARGET_DIR
[ -n "$CARGO_HOME" ] && export CARGO_HOME

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

# rustup's llvm-tools live under the sysroot (`lib/rustlib/<host>/bin`), not
# on PATH. cargo-llvm-cov finds them; a naive `command -v llvm-cov` does not
# (CI Coverage failed with `llvm-cov not found` after the wrapper landed).
# Distro rustc has no rustup; point at system binaries via PATH / LLVM_COV.
if command -v rustc >/dev/null 2>&1; then
    _sysroot="$(rustc --print sysroot 2>/dev/null || true)"
    _host="$(rustc -vV 2>/dev/null | awk '/^host: / { print $2; exit }')"
    _rustlib_bin="${_sysroot:+${_host:+$_sysroot/lib/rustlib/$_host/bin}}"
    if [ -n "${_rustlib_bin:-}" ] && [ -x "$_rustlib_bin/llvm-cov" ]; then
        PATH="$_rustlib_bin:$PATH"
        export PATH
    fi
    unset _sysroot _host _rustlib_bin
fi
if [ -z "${LLVM_COV:-}" ] && command -v llvm-cov >/dev/null 2>&1; then
    LLVM_COV="$(command -v llvm-cov)"
    export LLVM_COV
fi
if [ -z "${LLVM_PROFDATA:-}" ] && command -v llvm-profdata >/dev/null 2>&1; then
    LLVM_PROFDATA="$(command -v llvm-profdata)"
    export LLVM_PROFDATA
fi

# cargo-llvm-cov 0.9 rejects --target-dir. Default workspace
# `target/llvm-cov-target` is wiped by `actions/checkout` `git clean`.
# Put the compile cache next to the persistent CARGO_TARGET_DIR so
# instrumented rlibs survive the job (Coverage was a 22m cold rebuild when
# this lived in RUNNER_TEMP). Leftover *.profraw mixed floors to 54% —
# purge traces, do not rm -rf the rlibs.
if [ -z "${CARGO_LLVM_COV_TARGET_DIR:-}" ]; then
    if [ -n "${CARGO_TARGET_DIR:-}" ]; then
        CARGO_LLVM_COV_TARGET_DIR="${CARGO_TARGET_DIR%/}-llvm-cov"
    else
        CARGO_LLVM_COV_TARGET_DIR="$ROOT/target/llvm-cov-target"
    fi
    export CARGO_LLVM_COV_TARGET_DIR
fi

cov_cmd="cargo llvm-cov --workspace"
if [ -n "$COVERAGE_FEATURES" ]; then
    cov_cmd="$cov_cmd --features $COVERAGE_FEATURES"
fi
cov_cmd="$cov_cmd --ignore-filename-regex $IGNORE --summary-only -- --skip tests::watcher_picks_up_changes --skip picker_flow_over_real_index --skip launch_inherited_logins_retries_until_healthy --skip launch_isolated_home_and_tempdir_connect --skip launch_timeout_closes_stderr_then_hangs --skip launch_child_exit_is_startup_failed --skip launch_unsupported_version_does_not_retry --skip isolated_cwd_points_at_home_and_restores --skip git_log_status_diff_blame_and_commit_on_repo --skip provider_and_model_dialogs_load_custom_from_isolated_home --skip fill_model_catalog_from_disk_is_a_noop_when_config_load_fails"

report_cmd="cargo llvm-cov report --json --ignore-filename-regex $CRATE_IGNORE --summary-only"
floors_cmd="python3 scripts/check_coverage_floors.py $REPORT_JSON"

if [ "$dry_run" -eq 1 ]; then
    if [ -n "${CARGO_LLVM_COV_TARGET_DIR:-}" ]; then
        say "+ CARGO_LLVM_COV_TARGET_DIR=$CARGO_LLVM_COV_TARGET_DIR"
        say "+ LLVM_PROFILE_FILE=$CARGO_LLVM_COV_TARGET_DIR/whycodes-%p-%m.profraw"
    fi
    say "+ cargo llvm-cov clean --workspace --profraw-only"
    say "+ purge *.profraw *.profdata from compile cache (keep rlibs)"
    say "+ $cov_cmd"
    say "+ LLVM_COV_FLAGS=-skip-expansions $report_cmd > $REPORT_JSON"
    say "+ FAIL_UNDER=$FAIL_UNDER $floors_cmd"
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
# Drop coverage traces only. Never `llvm-cov clean` / `rm -rf` the compile cache.
cov_root="$CARGO_LLVM_COV_TARGET_DIR"
mkdir -p "$cov_root"
# Isolate script stamps CARGO_TARGET_DIR. The llvm-cov cache is
# `${CARGO_TARGET_DIR}-llvm-cov` and needs the same Cargo signature or
# `llvm-cov clean` refuses it (`invalid signature`) and leftover traces
# mix expansion dumps into the next JSON floors. Write via env so a
# CRLF-checked-out script cannot put CR in the filename.
stamp_cachedir() {
    _dir="$1"
    [ -n "$_dir" ] || return 0
    mkdir -p "$_dir"
    printf '%s\n' \
        'Signature: 8a477f597d28d172789f06886806bc55' \
        '# This file is a cache directory tag created by cargo.' \
        '# For information about cache directory tags see https://bford.info/cachedir/' \
        >"$_dir/CACHEDIR.TAG"
    unset _dir
}
stamp_cachedir "$cov_root"
# cargo-llvm-cov clean still looks at \$CARGO_TARGET_DIR/coverage-llvm-cov
# even when CARGO_LLVM_COV_TARGET_DIR is the sibling *-llvm-cov dir.
if [ -n "${CARGO_TARGET_DIR:-}" ]; then
    stamp_cachedir "${CARGO_TARGET_DIR%/}/coverage-llvm-cov"
fi
cargo llvm-cov clean --workspace --profraw-only >/dev/null 2>&1 || true
find "$cov_root" \( -name '*.profraw' -o -name '*.profdata' \) -delete
if [ -n "${CARGO_TARGET_DIR:-}" ] && [ -d "$CARGO_TARGET_DIR" ]; then
    find "$CARGO_TARGET_DIR" \( -name '*.profraw' -o -name '*.profdata' \) -delete
fi
export LLVM_PROFILE_FILE="$cov_root/whycodes-%p-%m.profraw"

set -- cargo llvm-cov --workspace
if [ -n "$COVERAGE_FEATURES" ]; then
    set -- "$@" --features "$COVERAGE_FEATURES"
fi
set -- "$@" \
    --ignore-filename-regex "$IGNORE" \
    --summary-only \
    -- \
    --skip tests::watcher_picks_up_changes \
    --skip picker_flow_over_real_index \
    --skip launch_inherited_logins_retries_until_healthy \
    --skip launch_isolated_home_and_tempdir_connect \
    --skip launch_timeout_closes_stderr_then_hangs \
    --skip launch_child_exit_is_startup_failed \
    --skip launch_unsupported_version_does_not_retry \
    --skip isolated_cwd_points_at_home_and_restores \
    --skip git_log_status_diff_blame_and_commit_on_repo \
    --skip provider_and_model_dialogs_load_custom_from_isolated_home \
    --skip fill_model_catalog_from_disk_is_a_noop_when_config_load_fails
run "$@"

# JSON floors need `-skip-expansions` on `llvm-cov export`. rustup llvm-cov
# 21 rejects that flag on `show` (workspace text report). cargo-llvm-cov
# reads LLVM_COV_FLAGS in the *parent* and passes them as argv to llvm-cov
# (the child env is stripped). Double-dash skip-expansions is rejected.
# Do not set this for the workspace run above.
LLVM_COV_FLAGS=-skip-expansions
export LLVM_COV_FLAGS
run cargo llvm-cov report --json --ignore-filename-regex "$CRATE_IGNORE" --summary-only >"$REPORT_JSON"
unset LLVM_COV_FLAGS
FAIL_UNDER="$FAIL_UNDER" python3 scripts/check_coverage_floors.py "$REPORT_JSON"
