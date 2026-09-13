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
need "purge *.profraw *.profdata from compile cache (keep rlibs)" "$dry"
need "cargo llvm-cov clean --workspace --profraw-only" "$dry"
forbid "rm -rf \$CARGO_LLVM_COV_TARGET_DIR" "$dry"
forbid "cargo llvm-cov clean --workspace --no-profraw" "$dry"
forbid "--fail-under-lines" "$dry"
need "FAIL_UNDER=82" "$dry"
need "--skip tests::watcher_picks_up_changes" "$dry"
need "--skip picker_flow_over_real_index" "$dry"
need "--skip launch_inherited_logins_retries_until_healthy" "$dry"
need "--skip isolated_cwd_points_at_home_and_restores" "$dry"
need "--skip git_log_status_diff_blame_and_commit_on_repo" "$dry"
need "--skip provider_and_model_dialogs_load_custom_from_isolated_home" "$dry"
need "--skip fill_model_catalog_from_disk_is_a_noop_when_config_load_fails" "$dry"
need "check_coverage_floors.py" "$dry"
need "/tmp/cov.json" "$dry"
need "LLVM_COV=scripts/llvm_cov_skip_expansions.sh LLVM_PROFDATA=" "$dry"
forbid "--features" "$dry"

dryf="$(COVERAGE_FEATURES=whycodes-storage/bundled "$SCRIPT" --dry-run)"
need "--features whycodes-storage/bundled" "$dryf"
forbid "--fail-under-lines" "$dryf"

dry100="$(FAIL_UNDER=100 "$SCRIPT" --dry-run)"
need "FAIL_UNDER=100" "$dry100"

dryjson="$(REPORT_JSON=/tmp/custom-cov.json "$SCRIPT" --dry-run)"
need "/tmp/custom-cov.json" "$dryjson"

# RUNNER_TEMP must not steal the compile cache (that was a 22m cold rebuild).
# Traces are purged in-place; instrumented rlibs stay next to CARGO_TARGET_DIR.
drytgt="$(
    env -u RUNNER_TEMP -u CARGO_LLVM_COV_TARGET_DIR \
        CARGO_TARGET_DIR=/tmp/pinned-llvm-target \
        "$SCRIPT" --dry-run
)"
need "CARGO_LLVM_COV_TARGET_DIR=/tmp/pinned-llvm-target-llvm-cov" "$drytgt"
forbid "--target-dir" "$drytgt"

dryci="$(
    env -u CARGO_LLVM_COV_TARGET_DIR \
        RUNNER_TEMP=/tmp/gha-runner-temp \
        CARGO_TARGET_DIR=/tmp/pinned-llvm-target \
        "$SCRIPT" --dry-run
)"
need "CARGO_LLVM_COV_TARGET_DIR=/tmp/pinned-llvm-target-llvm-cov" "$dryci"
need "LLVM_PROFILE_FILE=/tmp/pinned-llvm-target-llvm-cov/whycodes-%p-%m.profraw" "$dryci"
forbid "CARGO_LLVM_COV_TARGET_DIR=/tmp/gha-runner-temp/llvm-cov-target" "$dryci"

# rustup llvm-cov is under rustlib/bin, not PATH. The wrapper must prepend
# that dir so CI (and rustup clones) do not fail with `llvm-cov not found`.
src="$(cat "$SCRIPT")"
need "lib/rustlib/" "$src"
need 'rustc --print sysroot' "$src"
need "llvm_cov_skip_expansions.sh" "$src"
forbid 'rm -rf "$cov_root"' "$src"

wrap="$ROOT/scripts/llvm_cov_skip_expansions.sh"
need "export" "$(cat "$wrap")"
need "-skip-expansions" "$(cat "$wrap")"
sh -n "$wrap"
test -x "$wrap" || chmod +x "$wrap"

fake="$(mktemp)"
trap 'rm -f "$fake"' EXIT
printf '#!/bin/sh\nprintf %%s\\n "$*"\n' >"$fake"
chmod +x "$fake"
got="$(WHYCODES_LLVM_COV_REAL="$fake" "$wrap" export --summary-only)"
printf '%s\n' "$got" | grep -F -q -- "-skip-expansions" || {
    printf 'error: wrapper did not inject -skip-expansions on export\n' >&2
    printf '%s\n' "$got" >&2
    exit 1
}
got="$(WHYCODES_LLVM_COV_REAL="$fake" "$wrap" show --summary-only)"
if printf '%s\n' "$got" | grep -F -q -- "-skip-expansions"; then
    printf 'error: wrapper must not inject -skip-expansions on show\n' >&2
    printf '%s\n' "$got" >&2
    exit 1
fi
got="$(WHYCODES_LLVM_COV_REAL="$fake" "$wrap" export -skip-expansions --summary-only)"
n="$(printf '%s\n' "$got" | tr ' ' '\n' | grep -c -- '-skip-expansions' || true)"
if [ "$n" -ne 1 ]; then
    printf 'error: wrapper duplicated -skip-expansions (n=%s)\n' "$n" >&2
    printf '%s\n' "$got" >&2
    exit 1
fi

if "$SCRIPT" --nope >/dev/null 2>&1; then
    printf 'error: unknown argument should fail\n' >&2
    exit 1
fi

printf 'ok\n'
