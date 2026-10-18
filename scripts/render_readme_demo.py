#!/usr/bin/env python3
"""Render the README's animated terminal, `assets/tui-demo.svg`.

The frames come from the real TUI: `readme_demo_frames` in
`crates/tui/src/ui/readme_demo_tests.rs` paints a scripted turn through the
renderer and dumps every cell. This script runs that test and turns the dump
into one SVG that plays the frames on a loop with CSS only (GitHub shows SVG
through `<img>`, which allows CSS animation but no script).

Box-drawing and block cells become vector shapes rather than font glyphs, so
borders stay continuous whatever monospace font the viewer has. An element that
stays put across frames is written once and shown for its whole span, which
keeps the file small. Viewers asking for reduced motion get the last frame.

    python scripts/render_readme_demo.py            # writes assets/tui-demo.svg
    python scripts/render_readme_demo.py --frames f.json   # reuse a dump
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
from html import escape
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "assets" / "tui-demo.svg"

CW, CH = 9, 20  # cell size in px
PAD_X, TOP, PAD_B = 20, 48, 14  # window padding; TOP includes the title bar
BAR_H = 40
STROKE = 1.4
RADIUS = 4.0
TITLE = "whycodes — ~/my-project"
FONT = "'JetBrains Mono', 'Fira Code', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace"

HORIZ = "─"
VERT = "│"
CORNERS = "╭╮╰╯"
BLOCKS = {"█": (0.0, 1.0), "▀": (0.0, 0.5), "▄": (0.5, 1.0)}


def dump_frames(path: Path) -> None:
    env = dict(os.environ, WHYCODES_DEMO_FRAMES=str(path))
    cmd = [
        "cargo", "test", "-p", "whycodes-tui", "--lib", "readme_demo_frames",
        "--", "--ignored", "--exact", "ui::readme_demo_tests::readme_demo_frames",
    ]
    subprocess.run(cmd, cwd=ROOT, env=env, check=True)


def fmt(v: float) -> str:
    return f"{v:.1f}".rstrip("0").rstrip(".")


def frame_elements(frame: dict, fg0: str, bg0: str) -> list[str]:
    """SVG elements for one frame, each a self-contained string."""
    cells = frame["cells"]
    out: list[str] = []

    def cx(c: int) -> float:
        return PAD_X + c * CW

    def cy(r: int) -> float:
        return TOP + r * CH

    # Background bands (selection, diff lines, prompt band).
    for r, row in enumerate(cells):
        c = 0
        while c < len(row):
            bg = row[c][2] or bg0
            start = c
            while c < len(row) and (row[c][2] or bg0) == bg:
                c += 1
            if bg != bg0:
                out.append(
                    f'<rect x="{cx(start)}" y="{cy(r)}" width="{(c - start) * CW}" '
                    f'height="{CH}" fill="{bg}"/>'
                )

    # Horizontal border runs and corners.
    for r, row in enumerate(cells):
        mid = cy(r) + CH / 2
        c = 0
        while c < len(row):
            sym, fg = row[c][0], row[c][1] or fg0
            if sym == HORIZ:
                start = c
                while c < len(row) and row[c][0] == HORIZ and (row[c][1] or fg0) == fg:
                    c += 1
                out.append(
                    f'<path d="M {fmt(cx(start))} {fmt(mid)} H {fmt(cx(c))}" stroke="{fg}"/>'
                )
                continue
            if sym in CORNERS:
                x0, x1, xm = cx(c), cx(c + 1), cx(c) + CW / 2
                y0, y1 = cy(r), cy(r + 1)
                k = RADIUS
                d = {
                    "╭": f"M {fmt(x1)} {fmt(mid)} H {fmt(xm + k)} A {k} {k} 0 0 0 {fmt(xm)} {fmt(mid + k)} V {fmt(y1)}",
                    "╮": f"M {fmt(x0)} {fmt(mid)} H {fmt(xm - k)} A {k} {k} 0 0 1 {fmt(xm)} {fmt(mid + k)} V {fmt(y1)}",
                    "╰": f"M {fmt(xm)} {fmt(y0)} V {fmt(mid - k)} A {k} {k} 0 0 0 {fmt(xm + k)} {fmt(mid)} H {fmt(x1)}",
                    "╯": f"M {fmt(xm)} {fmt(y0)} V {fmt(mid - k)} A {k} {k} 0 0 1 {fmt(xm - k)} {fmt(mid)} H {fmt(x0)}",
                }[sym]
                out.append(f'<path d="{d}" stroke="{fg}"/>')
            c += 1

    # Vertical border runs, merged down the rows.
    for c in range(len(cells[0])):
        r = 0
        while r < len(cells):
            sym, fg = cells[r][c][0], cells[r][c][1] or fg0
            if sym != VERT:
                r += 1
                continue
            start = r
            while r < len(cells) and cells[r][c][0] == VERT and (cells[r][c][1] or fg0) == fg:
                r += 1
            x = cx(c) + CW / 2
            out.append(f'<path d="M {fmt(x)} {fmt(cy(start))} V {fmt(cy(r))}" stroke="{fg}"/>')

    # Block glyphs (the logo mark) as rects, merged along the row.
    for r, row in enumerate(cells):
        c = 0
        while c < len(row):
            sym, fg = row[c][0], row[c][1] or fg0
            if sym not in BLOCKS:
                c += 1
                continue
            start = c
            while c < len(row) and row[c][0] == sym and (row[c][1] or fg0) == fg:
                c += 1
            top, bot = BLOCKS[sym]
            out.append(
                f'<rect x="{cx(start)}" y="{fmt(cy(r) + top * CH)}" width="{(c - start) * CW}" '
                f'height="{fmt((bot - top) * CH)}" fill="{fg}"/>'
            )

    # Text runs: same style, single spaces allowed inside a run.
    special = set(HORIZ + VERT + CORNERS) | set(BLOCKS)
    for r, row in enumerate(cells):
        runs: list[list] = []  # [start_col, end_col, fg, bold, chars]
        for c, (sym, fg, _bg, bold) in enumerate(row):
            if sym in ("", " ") or sym in special:
                continue
            fg = fg or fg0
            last = runs[-1] if runs else None
            if last and last[2] == fg and last[3] == bold and c - last[1] <= 2:
                last[4] += " " * (c - last[1] - 1) + sym
                last[1] = c
            else:
                runs.append([c, c, fg, bold, sym])
        for start, end, fg, bold, text in runs:
            weight = ' font-weight="700"' if bold else ""
            out.append(
                f'<text x="{cx(start)}" y="{cy(r) + 14}" fill="{fg}"{weight} '
                f'textLength="{(end - start + 1) * CW}">{escape(text)}</text>'
            )

    caret = frame.get("caret")
    if caret:
        x, y = caret
        out.append(
            f'<rect class="caret" x="{cx(x)}" y="{cy(y) + 2}" width="{CW}" height="{CH - 4}" fill="{fg0}"/>'
        )
    return out


def dominant(frames: list[dict], idx: int) -> str:
    """Most common colour in the last frame; fg counts only painted glyphs."""
    counts: dict[str, int] = {}
    for row in frames[-1]["cells"]:
        for cell in row:
            if cell[idx] and (idx == 2 or cell[0].strip()):
                counts[cell[idx]] = counts.get(cell[idx], 0) + 1
    return max(counts, key=counts.get)


def render(doc: dict) -> str:
    frames = doc["frames"]
    cols, rows = doc["cols"], doc["rows"]
    fg0, bg0 = dominant(frames, 1), dominant(frames, 2)
    width = PAD_X * 2 + cols * CW
    height = TOP + rows * CH + PAD_B

    total = sum(f["hold_ms"] for f in frames)
    starts, t = [], 0
    for f in frames:
        starts.append(t)
        t += f["hold_ms"]

    # element -> frame indices where it is drawn
    seen: dict[str, list[int]] = {}
    for i, f in enumerate(frames):
        for el in dict.fromkeys(frame_elements(f, fg0, bg0)):
            seen.setdefault(el, []).append(i)

    # group elements by contiguous frame span
    spans: dict[tuple[int, int], list[str]] = {}
    for el, idxs in seen.items():
        a = prev = idxs[0]
        for i in idxs[1:] + [None]:
            if i is not None and i == prev + 1:
                prev = i
                continue
            spans.setdefault((a, prev), []).append(el)
            if i is not None:
                a = prev = i

    def pct(ms: int) -> str:
        return f"{ms * 100 / total:.3f}".rstrip("0").rstrip(".") + "%"

    last = len(frames) - 1
    css = [
        f"text{{font-size:15px;white-space:pre}}",
        f"path{{fill:none;stroke-width:{STROKE};stroke-linecap:square}}",
        f"g.a{{opacity:0;animation:{total}ms step-end infinite}}",
        ".caret{animation:1s step-end infinite blink}",
        "@keyframes blink{0%{opacity:1}50%{opacity:0}}",
    ]
    body = []
    n = 0
    for (a, b), els in sorted(spans.items()):
        if (a, b) == (0, last):
            body.extend(els)
            continue
        name = f"k{n}"
        n += 1
        on, off = starts[a], starts[b] + frames[b]["hold_ms"]
        keys = [f"0%{{opacity:{1 if a == 0 else 0}}}"]
        if a != 0:
            keys.append(f"{pct(on)}{{opacity:1}}")
        if off < total:
            keys.append(f"{pct(off)}{{opacity:0}}")
        keys.append(f"100%{{opacity:{1 if off >= total else 0}}}")
        css.append(f"@keyframes {name}{{{''.join(keys)}}}")
        cls = "a z" if b == last else "a"
        body.append(f'<g class="{cls}" style="animation-name:{name}">' + "".join(els) + "</g>")
    css.append(
        "@media (prefers-reduced-motion:reduce){g.a{animation:none}g.z{opacity:1}.caret{animation:none}}"
    )

    head = f"""<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" font-family="{escape(FONT)}" role="img" aria-label="whycodes TUI: a prompt is typed, the agent searches, reads, edits and runs tests, then replies">
<style>{''.join(css)}</style>
<rect width="{width}" height="{height}" rx="12" fill="{bg0}"/>
<rect x="0.5" y="0.5" width="{width - 1}" height="{height - 1}" rx="12" fill="none" stroke="#26262f"/>
<path d="M 0 {BAR_H} V 12 A 12 12 0 0 1 12 0 H {width - 12} A 12 12 0 0 1 {width} 12 V {BAR_H} Z" fill="#17171f"/>
<circle cx="26" cy="20" r="6.5" fill="#ff5f57"/>
<circle cx="48" cy="20" r="6.5" fill="#febc2e"/>
<circle cx="70" cy="20" r="6.5" fill="#28c840"/>
<text x="{width // 2}" y="25" text-anchor="middle" fill="#7d7d88" style="font-size:13px">{escape(TITLE)}</text>
"""
    return head + "\n".join(body) + "\n</svg>\n"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--frames", type=Path, help="existing frame dump (skips cargo)")
    ap.add_argument("--out", type=Path, default=OUT)
    args = ap.parse_args()

    if args.frames:
        doc = json.loads(args.frames.read_text(encoding="utf-8"))
    else:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "frames.json"
            dump_frames(path)
            doc = json.loads(path.read_text(encoding="utf-8"))

    svg = render(doc)
    args.out.write_text(svg, encoding="utf-8", newline="\n")
    print(f"{args.out.relative_to(ROOT) if args.out.is_relative_to(ROOT) else args.out}: "
          f"{len(doc['frames'])} frames, {len(svg.encode()) // 1024} KiB")
    return 0


if __name__ == "__main__":
    sys.exit(main())
