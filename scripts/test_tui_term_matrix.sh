#!/usr/bin/env sh
# Offline checks for scripts/tui_term_matrix.sh (no emulator windows).

set -eu

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
SCRIPT="$ROOT/scripts/tui_term_matrix.sh"

need() {
    printf '%s\n' "$2" | grep -F -q "$1" || {
        printf 'error: output missing %s\n' "$1" >&2
        printf '%s\n' "$2" >&2
        exit 1
    }
}

list="$("$SCRIPT" --list)"
need "host" "$list"
need "alacritty" "$list"
need "konsole" "$list"

dry="$(BIN=/tmp/whycode-fake DIR=/tmp/demo "$SCRIPT" --dry-run --no-build alacritty 2>&1)"
need "alacritty -e /tmp/whycode-fake -d /tmp/demo" "$dry"
need "launched=1" "$dry"

if "$SCRIPT" --dry-run --no-build not-a-term >/dev/null 2>&1; then
    printf 'error: unknown host should fail\n' >&2
    exit 1
fi

help="$("$SCRIPT" --help)"
need "scripts/tui_term_matrix.sh --list" "$help"
printf '%s\n' "$help" | grep -q '^set ' && {
    printf 'error: --help leaked script body\n' >&2
    exit 1
}

printf 'ok\n'
