# Phase 8 — TUI rendering and readability

**Status:** done (2026-07-31), except the render-cost criterion · **Depends on:** nothing · **Blocks:** nothing

Derived from [anomalyco/opencode](https://github.com/anomalyco/opencode), whose
TUI is the target look and feel.

## What can and cannot be taken

opencode's TUI is SolidJS on OpenTUI, running under Bun — 119 `.tsx` files.
whycodes's is Rust on ratatui. **No code transfers.** What transfers is the
design: which surfaces exist, how a message is rendered, and how themes are
defined.

One artefact does transfer directly: opencode ships 33 themes as JSON with a
published schema (`https://opencode.ai/theme.json`), MIT licensed. Adopting that
schema means those files work in whycodes unmodified.

## The gap

| | whycodes | opencode |
|---|---|---|
| Assistant output | plain text | markdown + tree-sitter syntax highlighting |
| Themes | 29, hardcoded in 1,161 lines of Rust | 33 JSON files against a schema |
| Dialogs | alert, confirm, help, provider | + session list, model, status, export, timeline, fork, subagent, workspace |
| Permission UI | y/n dialog | a dedicated view (681 lines) |
| Notifications | status line only | toasts |

The first row is the one that matters most, and it is nearly free:
`crates/format` already contains `markdown.rs` (163 lines) and `highlight.rs`
(64 lines), and `crates/tui/Cargo.toml` already declares `whycodes-format` as a
dependency — but `whycodes_format` does not appear anywhere under
`crates/tui/src`. The rendering code exists, is paid for in the dependency
graph, and is not called. Model output goes to the screen as raw
`Span::raw(line)`.

## Goal

Assistant output is readable — headings, lists, emphasis and fenced code are
rendered rather than printed as literal markup — and themes are data rather
than code.

## Scope

In:

- Wire `whycodes-format`'s markdown renderer into the chat view.
- Syntax highlighting for fenced code blocks.
- Load themes from JSON using opencode's schema, keeping the built-in set as a
  fallback so no configuration is required.
- A model picker dialog and a session list dialog, matching the two most-used
  entries in opencode's inventory.
- Toast notifications for transient events that currently overwrite the status
  line.

Out:

- Mermaid rendering. jcode spends 10k lines on it; the audience is small.
- Terminal image protocols (kitty, iterm2, sixel).
- Tree-sitter. opencode carries per-language query files and a parser
  registry. A grammar-free highlighter is enough for fenced blocks, and the
  decision is recorded below.
- Animations and background pulse effects.
- Rewriting the layout. The current chat/sidebar/prompt arrangement stays.

## Decisions to record before starting

- **Highlighter.** `syntect` (Sublime grammars, ~2 MB of binary, good
  coverage) versus a hand-rolled tokeniser for the ten languages that matter.
  Phase 5's binary-size budget applies; pick after measuring.
- **Theme loading.** Built-ins stay compiled in so a fresh install has themes
  with no files present. JSON is an override layer, read from the config
  directory.

## Tasks

- [x] Render assistant messages through `whycodes_format::markdown`
- [x] Fenced code blocks: language tag, syntax colours, and a copyable body
- [x] Confirm the dependency is now genuinely used, or drop it
- [x] Theme JSON schema matching opencode's `defs` + `theme` indirection,
      including its light/dark pairing
- [x] Load `~/.config/…/themes/*.json`, falling back to the built-ins
- [x] Validate a theme file on load and report the offending key, not just
      "invalid"
- [x] Model picker dialog, bound to `/models` with no argument
- [x] Session list dialog, bound to `/sessions` in the TUI (today it only
      exists in the `--plain` REPL)
- [x] Toasts for transient notices, so the status line stops being a
      dumping ground
- [x] README: document theme files and where they are read from

## Acceptance criteria

- [x] A response containing headings, lists, bold, inline code and a fenced
      block renders with none of the markup characters visible
- [x] A fenced block tagged `rust` is coloured; an untagged one still renders
      as a block rather than as prose
- [x] Markdown rendering does not break the existing streaming path — partial
      output during a turn stays readable
- [x] An opencode theme JSON file, taken unmodified, loads and applies
- [x] A malformed theme file reports the bad key and falls back rather than
      panicking
- [x] Every built-in theme still resolves after the loader change — the
      existing `ThemeName::ALL` round-trip test keeps passing
- [x] The model picker lists configured providers and switching updates the
      header
- [ ] Rendering cost does not regress startup or idle draws beyond Phase 5's
      ceilings
- [x] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --
      -D warnings`, `cargo test --workspace` all clean

## Risks

- **Streaming and markdown conflict.** Markdown needs a complete block to
  render; streaming delivers partial text. Rendering the incomplete tail as
  plain text and re-rendering on block completion is the usual answer, and it
  is the part most likely to flicker.
- **Highlighter weight.** `syntect` carries grammar data. If it moves the
  binary materially, the hand-rolled option wins.
- **Theme schema drift.** Adopting someone else's schema means their changes
  can break our loader. Pin to the schema as it exists, validate, and fall
  back rather than fail.

## Reference

`opencode/packages/tui/src/theme/assets/*.json` — 33 themes and the schema.
`opencode/packages/ui/src/marked.tsx` (549 lines) — their markdown renderer.
`opencode/packages/tui/src/parsers-config.ts` — the tree-sitter setup this
phase deliberately does not copy.
`opencode/packages/tui/src/ui/dialog-select.tsx` (748) — the generic select
dialog every picker is built from.
