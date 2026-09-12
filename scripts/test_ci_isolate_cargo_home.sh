#!/usr/bin/env sh
# Offline checks for scripts/ci_isolate_cargo_home.sh and ci.yml cache wiring.

set -eu

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
SCRIPT="$ROOT/scripts/ci_isolate_cargo_home.sh"
WF="$ROOT/.github/workflows/ci.yml"
COV="$ROOT/scripts/coverage.sh"

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

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

envfile="$tmp/github.env"
: >"$envfile"
out1="$(
    HOME="$tmp/home" \
        RUNNER_TOOL_CACHE="$tmp/tools" \
        RUNNER_NAME="1panel-ci-2" \
        GITHUB_JOB="test" \
        GITHUB_ENV="$envfile" \
        "$SCRIPT"
)"
need "CARGO_HOME=" "$out1"
need "CARGO_TARGET_DIR=" "$out1"
need "CARGO_CACHE_HIT_LOCAL=0" "$out1"
need "1panel-ci-2" "$out1"
need "/test" "$out1"
forbid "/.cargo" "$out1"
grep -q '^CARGO_HOME=' "$envfile" || {
    printf 'error: GITHUB_ENV missing CARGO_HOME\n' >&2
    exit 1
}
grep -q '^CARGO_TARGET_DIR=' "$envfile" || {
    printf 'error: GITHUB_ENV missing CARGO_TARGET_DIR\n' >&2
    exit 1
}

home1="$(printf '%s\n' "$out1" | sed -n 's/^CARGO_HOME=//p')"
target1="$(printf '%s\n' "$out1" | sed -n 's/^CARGO_TARGET_DIR=//p')"
[ -d "$home1" ] && [ -d "$target1" ] || {
    printf 'error: homes were not created\n' >&2
    exit 1
}
[ -f "$target1/CACHEDIR.TAG" ] || {
    printf 'error: CARGO_TARGET_DIR missing CACHEDIR.TAG\n' >&2
    exit 1
}
grep -q 'Signature: 8a477f597d28d172789c096e48218643' "$target1/CACHEDIR.TAG" || {
    printf 'error: CACHEDIR.TAG signature missing\n' >&2
    exit 1
}
[ "$home1" != "$target1" ] || {
    printf 'error: CARGO_HOME must not equal CARGO_TARGET_DIR\n' >&2
    exit 1
}

mkdir -p "$home1/registry/index"
out_hit="$(
    HOME="$tmp/home" \
        RUNNER_TOOL_CACHE="$tmp/tools" \
        RUNNER_NAME="1panel-ci-2" \
        GITHUB_JOB="test" \
        "$SCRIPT"
)"
need "CARGO_CACHE_HIT_LOCAL=1" "$out_hit"

out2="$(
    HOME="$tmp/home" \
        RUNNER_TOOL_CACHE="$tmp/tools" \
        RUNNER_NAME="1panel-ci-2" \
        GITHUB_JOB="coverage" \
        "$SCRIPT"
)"
need "/coverage" "$out2"
if [ "$out1" = "$out2" ]; then
    printf 'error: test vs coverage must not share CARGO_HOME\n' >&2
    printf '%s\n' "$out1" "$out2" >&2
    exit 1
fi

out3="$(
    HOME="$tmp/home" \
        RUNNER_TOOL_CACHE="$tmp/tools" \
        RUNNER_NAME="1panel-ci-3" \
        GITHUB_JOB="test" \
        "$SCRIPT"
)"
need "1panel-ci-3" "$out3"
home3="$(printf '%s\n' "$out3" | sed -n 's/^CARGO_HOME=//p')"
[ "$home1" != "$home3" ] || {
    printf 'error: two listeners must not share CARGO_HOME\n' >&2
    exit 1
}

# Unset RUNNER_TOOL_CACHE: CI jobs inherit it, which would skip the
# ~/.cache fallback this case is meant to cover.
out4="$(
    env -u RUNNER_TOOL_CACHE \
        HOME="$tmp/home" \
        RUNNER_NAME="local" \
        GITHUB_JOB="lint" \
        "$SCRIPT"
)"
need ".cache/whycodes-ci-cargo" "$out4"
need ".cache/whycodes-ci-target" "$out4"
forbid "${tmp}/home/.cargo" "$out4"

wf="$(cat "$WF")"
need "scripts/ci_isolate_cargo_home.sh" "$wf"
need "cache-targets: false" "$wf"
need "CARGO_CACHE_HIT_LOCAL" "$wf"
need "shared-key: whycodes-ci-registry" "$wf"
forbid 'RUNNER_TEMP/cargo-home' "$wf"

dry="$(CARGO_TARGET_DIR=/tmp/pinned-llvm-target "$COV" --dry-run)"
need "CARGO_LLVM_COV_TARGET_DIR=/tmp/pinned-llvm-target/llvm-cov-target" "$dry"
forbid "--target-dir" "$dry"

printf 'ok\n'