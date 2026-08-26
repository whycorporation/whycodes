#!/usr/bin/env sh
# Offline checks for scripts/pre-push (no network, no real git push).

set -eu

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
HOOK="$ROOT/scripts/pre-push"
ZERO=0000000000000000000000000000000000000000
SHA=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa

ok() {
    printf '%s\n' "$1" | "$HOOK" >/dev/null
}

bad() {
    if printf '%s\n' "$1" | "$HOOK" >/dev/null 2>&1; then
        printf 'error: expected failure for: %s\n' "$1" >&2
        exit 1
    fi
}

ok "refs/heads/main $SHA refs/heads/main $ZERO"
ok "refs/heads/main $SHA refs/heads/main $SHA"
ok "refs/tags/v0.1.0 $SHA refs/tags/v0.1.0 $ZERO"
ok "HEAD $SHA refs/heads/main $ZERO"

# Deletes (zero local sha) of any remote ref, including stray checkpoints.
# Git may send an empty local ref (leading space; `read` shifts) or the
# literal token `(delete)`.
ok "(delete) $ZERO refs/cline/checkpoints/x $SHA"
ok " $ZERO refs/cline/checkpoints/x $SHA"

bad "refs/cline/checkpoints/1 $SHA refs/cline/checkpoints/1 $ZERO"
bad "refs/stash $SHA refs/stash $ZERO"
bad "refs/notes/commits $SHA refs/notes/commits $ZERO"
bad "refs/heads/main $SHA refs/cline/checkpoints/1 $ZERO"

# Empty stdin (nothing to push) must succeed.
: | "$HOOK"

# Syntax: hook itself is a valid shell script.
sh -n "$HOOK"
sh -n "$ROOT/scripts/install_git_hooks.sh"

printf 'test_pre_push: ok\n'
