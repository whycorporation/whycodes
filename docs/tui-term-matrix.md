# TUI terminal matrix

`cargo test -p whycode-tui` paints into a ratatui buffer. It does **not**
exercise a real PTY or emulator CSI/OSC. Colour, mouse, clipboard, paste, and
keyboard-enhancement flags only show up in a host like Alacritty.

This page is the manual pass. Launch helpers live in
[`scripts/tui_term_matrix.sh`](../scripts/tui_term_matrix.sh).

## Launch

```bash
cargo build -p whycode-cli
scripts/tui_term_matrix.sh --list          # which hosts are on PATH
scripts/tui_term_matrix.sh                 # open every installed default host
scripts/tui_term_matrix.sh alacritty kitty # subset
```

Same binary in every window (`BIN=…` / `DIR=…` override). After a bad run:

```bash
tail -40 ~/.local/share/whycode/logs/unified.jsonl
```

Look for `tui.starting` (`stdin_tty` / `stdout_tty`), `tui.ready`
(`term_w` / `term_h`), `tui.first_frame`, `tui.size_fallback`,
`tui.loop_error`, `tui.exit`.

## Hosts

| Host | Family | Typical `TERM` | Notes |
|------|--------|----------------|-------|
| Alacritty | GPU | `alacritty` | OSC 52, kitty keyboard proto, truecolor |
| Kitty | GPU | `xterm-kitty` | Same + graphics; drag-drop as bracketed paste |
| WezTerm | GPU | `wezterm` | OSC 52, keyboard proto, truecolor |
| Ghostty | GPU | `xterm-ghostty` | Same class as Kitty/WezTerm |
| foot | Wayland | `foot` | Lean; OSC 52 usually on |
| GNOME Terminal / Ptyxis / Tilix | VTE | `xterm-256color` | OSC 52 often off; no kitty keyboard proto |
| Konsole | Qt | `konsole-256color` | Mouse/clipboard quirks vs VTE |

Minimum useful set: **Alacritty + Kitty + WezTerm + one VTE**. Add **foot** on
Wayland.

Whycode does not special-case `TERM` strings. Behaviour is whatever the
emulator implements. Setup is in `crates/tui/src/run.rs`: `/dev/tty`,
alt-screen, mouse capture, bracketed paste, and (when
`supports_keyboard_enhancement`) `DISAMBIGUATE_ESCAPE_CODES`.

## Checklist (every host)

Do these in the window that just opened. Mark fail + host in the PR / issue.

| # | Check | Pass |
|---|--------|------|
| 1 | First frame paints (home / prompt), no flash-and-quit | |
| 2 | `q` or `Ctrl+Q` leaves alt-screen; shell prompt, cursor, colours restore | |
| 3 | Theme looks 24-bit, not 256-colour banding (`COLORTERM=truecolor`) | |
| 4 | Drag-select copies trimmed text (not a row of pad spaces) | |
| 5 | **Shift+drag** still uses the host’s native selection | |
| 6 | Copy lands in the system clipboard (OSC 52 and/or `wl-copy` / `xclip`) | |
| 7 | Paste 5 lines at once → one block, not key-spam (bracketed paste) | |
| 8 | Drag a `.png` onto the window → image chip on the prompt | |
| 9 | **Shift+Enter** inserts a newline (GPU hosts). VTE: often unsupported | |
| 10 | Resize the window; layout reflows. 0×0 must not happen (`tui.size_fallback`) | |
| 11 | Hover chrome updates (message / button highlight) | |
| 12 | `?` help modal: scroll, select-copy, `[✗]` close | |

## Capability cuts (same emulator)

Reproduce “dumb host” without installing another terminal:

```bash
BIN="${BIN:-target/debug/whycode}"

# 256 colour, no COLORTERM
env -u COLORTERM TERM=xterm-256color "$BIN" -d .

# More primitive
TERM=xterm "$BIN" -d .

# Force the readline REPL
WHYCODE_PLAIN=1 "$BIN" -d .
# or: "$BIN" --plain -d .
```

`WHYCODE_PLAIN` / `--plain` must **not** open alt-screen.

## What CI already covers

| Layer | Command | Sees a real emulator? |
|-------|---------|------------------------|
| Widget / layout / clipboard string trim | `cargo test -p whycode-tui` | No |
| First-frame / RSS benches | `scripts/bench_*.py` | No (or tmux-ish PTY only) |
| CSI/OSC, mouse, Shift+Enter | this matrix | Yes |

Do not add “headless Alacritty” jobs to CI unless someone owns a Xephyr /
`xdotool` harness. `script(1)` and `tmux` give a PTY but lie about mouse and
clipboard.

## When you find a host bug

1. Confirm it is host-specific (same binary, second emulator).
2. Grab the JSONL slice around `tui.ready` / `tui.loop_error` / `tui.exit`.
3. Append a short entry to [`knowhow.md`](knowhow.md) if the fix is easy to
   reintroduce (event-loop return value, mouse dirty flag, `/dev/tty`, SIGPIPE).
