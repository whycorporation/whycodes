# Plan — Semantic / auto memory

**Status:** shipped (v1) · **Was:** phase 6 · **Depends on:** none (ONNX deferred)

## Decision (2026-08-05)

**v1 does not bundle ONNX MiniLM.** Local **feature-hash embeddings** (char
n-grams + tokens → dim 256, cosine top-k) give offline semantic recall with
near-zero RSS. MiniLM remains an optional future upgrade behind a feature flag.

Competitive alignment:

| Source | What we took |
|--------|----------------|
| **Claude Code** | `MEMORY.md` index always injected (200 lines / 25 KiB), human-editable, `/memory` |
| **Grok Build / Hindsight** | Auto-recall on each user prompt; knowledge tool for write/list/search |
| **jcode** | Auto inject by similarity (no tool call for recall); store beside sessions |

## Problem

Every whycode session starts cold. Facts established in one session are gone in
the next. The user re-explains, and pays tokens to do it.

## Goal

Facts persist across sessions and surface when relevant, without a network call
and without ONNX RSS cost.

## What shipped

- Crate `whycode-memory`: project key (git root), hashing embedder, inject, dual write
- SQLite `memories` table (migration in `whycode-storage`)
- On-disk `{data_dir}/memory/<project_key>/MEMORY.md` (Claude-style index)
- Auto-inject: capped MEMORY.md + top-k semantic hits on user turns
- Write: `/remember`, `whycode memory add`, `memory` tool (`write|list|search|delete`)
- CLI: `whycode memory list|search|add|delete|clear|path`
- Config `[memory]` (enabled default **true**), `--no-memory`, `WHYCODE_NO_MEMORY=1`
- Core tool profile includes `memory` (small schema; mutator is serial)

## Config

```toml
[memory]
enabled = true
auto_inject = true
max_index_lines = 200
max_index_bytes = 25600
recall_top_k = 5
recall_min_score = 0.28
recall_token_budget = 800
embed_dim = 256
```

## Acceptance (v1)

- [x] Embedding runs with the network disabled
- [x] Fact via `/remember` / `memory add` stored in SQLite + MEMORY.md
- [x] Semantic search ranks related text above unrelated (unit tests)
- [x] Injected blocks labeled (`# Auto Memory` / `# Recalled Memories`)
- [x] Token/char budget + MEMORY.md caps
- [x] `whycode memory delete` removes entry
- [x] `--no-memory` / `enabled=false` disables inject and writes
- [x] Workspace compiles; memory/storage tests pass

## Out of scope (later)

- LLM auto-extract retain loop (Hindsight-style post-turn)
- Hosted / cross-machine sync
- Codebase embedding / code RAG
- Bundled MiniLM ONNX
- Subagent-scoped memory banks

## Reference

Claude Code auto memory docs; Grok Build Hindsight plugin; jcode `jcode-embedding`.
