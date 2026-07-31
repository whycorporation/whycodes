#!/usr/bin/env sh
# Remove the whycode binary.
#
# Config and session data are left alone unless --purge is given: a user
# uninstalling to reinstall should not lose their providers and history.

set -eu

INSTALL_DIR="${WHYCODE_INSTALL_DIR:-$HOME/.local/bin}"
PURGE=0

for arg in "$@"; do
    case "$arg" in
        --purge) PURGE=1 ;;
        *) printf 'unknown option: %s\n' "$arg" >&2; exit 1 ;;
    esac
done

removed=0
if [ -f "$INSTALL_DIR/whycode" ]; then
    rm -f "$INSTALL_DIR/whycode"
    printf 'Removed %s\n' "$INSTALL_DIR/whycode"
    removed=1
else
    printf 'No binary at %s\n' "$INSTALL_DIR/whycode"
fi

if [ "$PURGE" -eq 1 ]; then
    for dir in \
        "${XDG_CONFIG_HOME:-$HOME/.config}/whycode" \
        "${XDG_DATA_HOME:-$HOME/.local/share}/whycode" \
        "$HOME/Library/Application Support/com.whycorporation.whycode" \
        "$HOME/Library/Caches/com.whycorporation.whycode"
    do
        if [ -d "$dir" ]; then
            rm -rf "$dir"
            printf 'Removed %s\n' "$dir"
            removed=1
        fi
    done
else
    printf 'Config and session data were kept. Pass --purge to remove them too.\n'
fi

[ "$removed" -eq 1 ] || printf 'Nothing to remove.\n'
