#!/usr/bin/env sh
# Launch whycode's TUI in popular Linux terminal emulators.
#
# cargo test does not cover emulator CSI/OSC (mouse, OSC 52, Shift+Enter,
# truecolor). This script only opens a real PTY per host; walk the checklist
# in docs/tui-term-matrix.md in each window.
#
# Usage:
#   scripts/tui_term_matrix.sh                 # build (if needed) + launch all
#   scripts/tui_term_matrix.sh --list          # which hosts are installed
#   scripts/tui_term_matrix.sh --dry-run       # print commands, do not exec
#   scripts/tui_term_matrix.sh --no-build alacritty kitty
#   BIN=/usr/local/bin/whycode scripts/tui_term_matrix.sh
#   DIR=/tmp/demo scripts/tui_term_matrix.sh

set -eu

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
DIR="${DIR:-$ROOT}"
BIN="${BIN:-$ROOT/target/debug/whycode}"
DOCS="docs/tui-term-matrix.md"

do_build=1
dry_run=0
list_only=0
hosts=""

say() { printf '%s\n' "$*"; }
err() { printf 'error: %s\n' "$*" >&2; }

usage() {
    sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'
}

have() { command -v "$1" >/dev/null 2>&1; }

# Default launch set: GPU emulators + foot + one VTE family.
DEFAULT_HOSTS="alacritty kitty wezterm ghostty foot gnome-terminal ptyxis"

# Print "exe<TAB>argv" for a host name. argv uses the tokens @BIN@ and @DIR@.
host_spec() {
    case "$1" in
        alacritty)      printf '%s\t%s\n' alacritty      '-e @BIN@ -d @DIR@' ;;
        kitty)          printf '%s\t%s\n' kitty          '@BIN@ -d @DIR@' ;;
        wezterm)        printf '%s\t%s\n' wezterm        'start --cwd @DIR@ -- @BIN@ -d @DIR@' ;;
        ghostty)        printf '%s\t%s\n' ghostty        '-e @BIN@ -d @DIR@' ;;
        foot)           printf '%s\t%s\n' foot           '@BIN@ -d @DIR@' ;;
        gnome-terminal) printf '%s\t%s\n' gnome-terminal '-- @BIN@ -d @DIR@' ;;
        ptyxis)         printf '%s\t%s\n' ptyxis         '-- @BIN@ -d @DIR@' ;;
        xfce4-terminal) printf '%s\t%s\n' xfce4-terminal '-x @BIN@ -d @DIR@' ;;
        tilix)          printf '%s\t%s\n' tilix          '-e @BIN@ -d @DIR@' ;;
        konsole)        printf '%s\t%s\n' konsole        '-e @BIN@ -d @DIR@' ;;
        *) return 1 ;;
    esac
}

all_names() {
    printf '%s\n' alacritty kitty wezterm ghostty foot \
        gnome-terminal ptyxis xfce4-terminal tilix konsole
}

expand_argv() {
    printf '%s\n' "$1" | sed "s|@BIN@|$BIN|g; s|@DIR@|$DIR|g"
}

while [ $# -gt 0 ]; do
    case "$1" in
        -h|--help) usage; exit 0 ;;
        --list) list_only=1 ;;
        --dry-run) dry_run=1 ;;
        --no-build) do_build=0 ;;
        --build) do_build=1 ;;
        --) shift; break ;;
        -*) err "unknown flag $1"; usage >&2; exit 2 ;;
        *)
            if [ -n "$hosts" ]; then
                hosts="$hosts $1"
            else
                hosts="$1"
            fi
            ;;
    esac
    shift
done

while [ $# -gt 0 ]; do
    if [ -n "$hosts" ]; then
        hosts="$hosts $1"
    else
        hosts="$1"
    fi
    shift
done

if [ -z "$hosts" ]; then
    hosts="$DEFAULT_HOSTS"
fi

if [ "$list_only" -eq 1 ]; then
    say "host            binary           status"
    for name in $(all_names); do
        spec="$(host_spec "$name")" || continue
        exe="${spec%%	*}"
        if have "$exe"; then
            st="ok"
        else
            st="missing"
        fi
        printf '%-15s %-16s %s\n' "$name" "$exe" "$st"
    done
    exit 0
fi

if [ "$dry_run" -eq 0 ] && [ ! -d "$DIR" ]; then
    err "DIR is not a directory: $DIR"
    exit 1
fi

if [ "$do_build" -eq 1 ] && [ "$dry_run" -eq 0 ]; then
    if [ ! -x "$BIN" ]; then
        say "building whycode-cli → $BIN"
        (cd "$ROOT" && cargo build -p whycode-cli)
    fi
fi

if [ "$dry_run" -eq 0 ] && [ ! -x "$BIN" ]; then
    err "binary not executable: $BIN (pass --build or BIN=...)"
    exit 1
fi

say "binary: $BIN"
say "cwd:    $DIR"
say "check:  $DOCS"
say ""

launched=0
skipped=0
unknown=0

for name in $hosts; do
    if ! spec="$(host_spec "$name")"; then
        err "unknown host: $name"
        unknown=$((unknown + 1))
        continue
    fi
    exe="${spec%%	*}"
    raw="${spec#*	}"
    argv="$(expand_argv "$raw")"

    # --dry-run prints the command even when the emulator is missing so CI
    # can lock argv without installing Alacritty / VTE on the runner.
    if [ "$dry_run" -eq 0 ] && ! have "$exe"; then
        say "skip  $name  ($exe not on PATH)"
        skipped=$((skipped + 1))
        continue
    fi

    say "→     $name  ($exe $argv)"
    if [ "$dry_run" -eq 0 ]; then
        # Intentional word-split of the host argv template.
        # shellcheck disable=SC2086
        "$exe" $argv &
    fi
    launched=$((launched + 1))
done

say ""
say "launched=$launched skipped=$skipped unknown=$unknown"
if [ "$dry_run" -eq 0 ] && [ "$launched" -gt 0 ]; then
    say "Walk $DOCS in each window. Logs: ~/.local/share/whycode/logs/unified.jsonl"
fi

if [ "$unknown" -gt 0 ]; then
    exit 2
fi
if [ "$launched" -eq 0 ]; then
    err "no terminal emulator launched (install one of: $DEFAULT_HOSTS)"
    exit 1
fi
