#!/usr/bin/env sh
# Offline checks for scripts/coverage.sh (no llvm-cov run).

set -eu

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
SCRIPT="$ROOT/scripts/coverage.sh"

need() {
    printf '%s\n' "$2" | grep -F -q -- "$1" || {
        printf 'error: output missing %s\n' "$1" >&2
        printf '%s\n' "$2" >&2
        exit 1
    }
}

forbid() {
    if printf '%s\n' "$2" | grep -F -q -- "$1"; then
        printf 'error: output should not contain %s\n' "$1" >&2
        printf '%s\n' "$2" >&2
        exit 1
    fi
}

help="$("$SCRIPT" --help)"
need "scripts/coverage.sh" "$help"
need "FAIL_UNDER" "$help"
need "COVERAGE_FEATURES" "$help"
printf '%s\n' "$help" | grep -q '^set ' && {
    printf 'error: --help leaked script body\n' >&2
    exit 1
}

dry="$("$SCRIPT" --dry-run)"
need "cargo llvm-cov --workspace" "$dry"
need "--fail-under-lines 82" "$dry"
need "--skip tests::watcher_picks_up_changes" "$dry"
need "--skip picker_flow_over_real_index" "$dry"
need "--skip launch_inherited_logins_retries_until_healthy" "$dry"
need "--skip isolated_cwd_points_at_home_and_restores" "$dry"
need "check_coverage_floors.py" "$dry"
need "/tmp/cov.json" "$dry"
forbid "--features" "$dry"

dryf="$(COVERAGE_FEATURES=whycodes-storage/bundled "$SCRIPT" --dry-run)"
need "--features whycodes-storage/bundled" "$dryf"
need "--fail-under-lines 82" "$dryf"

dry100="$(FAIL_UNDER=100 "$SCRIPT" --dry-run)"
need "--fail-under-lines 100" "$dry100"

dryjson="$(REPORT_JSON=/tmp/custom-cov.json "$SCRIPT" --dry-run)"
need "/tmp/custom-cov.json" "$dryjson"

# rustup llvm-cov is under rustlib/bin, not PATH. The wrapper must prepend
# that dir so CI (and rustup clones) do not fail with `llvm-cov not found`.
src="$(cat "$SCRIPT")"
need "lib/rustlib/" "$src"
need 'rustc --print sysroot' "$src"

if "$SCRIPT" --nope >/dev/null 2>&1; then
    printf 'error: unknown argument should fail\n' >&2
    exit 1
fi

printf 'ok\n'
