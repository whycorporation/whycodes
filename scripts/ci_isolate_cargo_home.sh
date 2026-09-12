#!/usr/bin/env sh
# Pin CARGO_HOME + CARGO_TARGET_DIR to directories that:
#   - survive job teardown (not RUNNER_TEMP / not the git worktree)
#   - are unique per Runner.Listener + GitHub job (not ~/.cargo)
#
# Three listeners share one OS user. A shared ~/.cargo/registry unpack
# produced strum_macros E0583 / missing rustversion .d files. A temp
# home avoided that but forced a cold crates.io fetch every job, and
# `actions/checkout` `git clean -ffdx` wiped workspace `target/`.
#
# Usage (CI): sh scripts/ci_isolate_cargo_home.sh
# Writes CARGO_HOME, CARGO_TARGET_DIR, CARGO_CACHE_HIT_LOCAL into
# GITHUB_ENV when that file is set; always prints CARGO_HOME.

set -eu

sanitize() {
    printf '%s' "$1" | tr -c 'A-Za-z0-9._-' '_'
}

job="$(sanitize "${GITHUB_JOB:-unknown}")"
runner="$(sanitize "${RUNNER_NAME:-local}")"

if [ -n "${RUNNER_TOOL_CACHE:-}" ]; then
    cargo_base="${RUNNER_TOOL_CACHE}/whycodes-cargo"
    target_base="${RUNNER_TOOL_CACHE}/whycodes-target"
else
    cargo_base="${HOME%/}/.cache/whycodes-ci-cargo"
    target_base="${HOME%/}/.cache/whycodes-ci-target"
fi

home="${cargo_base}/${runner}/${job}"
target="${target_base}/${runner}/${job}"

case "$home" in
    "${HOME%/}/.cargo"|"${HOME%/}/.cargo"/*)
        printf 'error: refusing shared ~/.cargo as CARGO_HOME (%s)\n' "$home" >&2
        exit 1
        ;;
esac

mkdir -p "$home" "$target"
# Cargo (and llvm-cov clean) require this exact first line. A heredoc from a
# CRLF-checked-out script produced "invalid signature"; printf is one LF.
printf 'Signature: 8a477f597d28d172789c096e48218643\n' >"$target/CACHEDIR.TAG"

if [ -d "$home/registry/index" ] || [ -d "$home/registry/src" ]; then
    hit=1
else
    hit=0
fi

printf 'CARGO_HOME=%s\n' "$home"
printf 'CARGO_TARGET_DIR=%s\n' "$target"
printf 'CARGO_CACHE_HIT_LOCAL=%s\n' "$hit"
if [ -n "${GITHUB_ENV:-}" ]; then
    {
        printf 'CARGO_HOME=%s\n' "$home"
        printf 'CARGO_TARGET_DIR=%s\n' "$target"
        printf 'CARGO_CACHE_HIT_LOCAL=%s\n' "$hit"
    } >>"$GITHUB_ENV"
fi
