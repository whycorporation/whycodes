# Plan — Semantic memory

**Status:** not started · **Was:** phase 6 · **Depends on:** performance residual comfort ([plan-performance.md](plan-performance.md)) · **Blocks:** nothing

Largest feature in this plan. Not started, deliberately.

## Why not started

Two of its preconditions are not met.

**Performance residual is not fully closed.** The reason for sequencing memory
after measurement was that the harness tells us whether the RSS cost is
acceptable. Startup/RSS/first-frame and idle draws are measured
([benchmarks.md](benchmarks.md)); multi-session cost of a *running* agent with
an embedding model still needs a deliberate baseline before landing memory.
Starting now without that check is the specific thing the sequencing was meant
to prevent.

**The model-distribution decision is open.** Bundling MiniLM roughly triples the
download; fetching it on first use adds a network dependency and a supply-chain
surface that then needs checksum pinning. That is the first task listed below
and it is a judgement about what the project wants to be, not something to
settle by picking whichever is easier to implement.

The honest position is that this phase is one substantial piece of work that
should be started deliberately, not begun because the phases before it happened
to finish. Everything below stands as written.

## Problem

Every whycode session starts cold. Facts established in one session — how this
project is built, which module owns what, a decision and its reasoning — are
gone in the next. The user re-explains, and pays tokens to do it.

The obvious implementation is a memory *tool* the model calls. That is what
makes it expensive: the tool definition sits in every request, and recall
depends on the model deciding to search.

jcode's approach avoids both. Text is embedded locally, and recall happens by
cosine similarity against the current context automatically, with no tool call
and no model decision. Their `jcode-embedding` crate is 572 lines and runs
`all-MiniLM-L6-v2` in-process through `tract-onnx` + `tokenizers` — no network,
no external service.

Note the trade honestly: jcode's headline memory figure is stated *with local
embeddings disabled*. An in-process ONNX model costs real RSS. The perf plan is what
lets us decide whether we accept that.

## Goal

Facts persist across sessions and surface when relevant, without a per-request
tool definition and without a network call.

## Scope

In:

- Local embedding of memory entries with a bundled ONNX model.
- Automatic recall: embed the current context, retrieve top-k by cosine
  similarity above a threshold, inject into the system prompt.
- Storage in the existing SQLite database, alongside sessions.
- Write path: explicit (`/remember`) first; automatic extraction later, gated
  on the explicit path working.
- `--no-memory` and a config switch, both defaulting to **off** until the
  memory cost from the perf harness justifies flipping it.
- Inspection: list, search and delete memory entries.

Out:

- A hosted or cross-machine memory service.
- Automatic extraction of memories from a transcript. Second iteration. Getting
  this wrong pollutes the store with noise that then degrades every recall.
- Embedding the whole codebase for retrieval. Different feature (code search),
  different design.
- Fine-tuning or training anything.

## Tasks

- [ ] Decide model distribution: bundled in the binary (size cost, ~90 MB for
      MiniLM ONNX) versus downloaded on first use (network dependency,
      integrity check needed). Record the decision here.
- [ ] `crates/embedding`: load the model, embed text, cosine similarity, top-k
- [ ] Schema migration: `memories(id, text, embedding BLOB, source_session,
      created_at, last_recalled_at, recall_count)`
- [ ] `/remember <text>` writing an entry
- [ ] Recall: embed the turn's context, retrieve top-k over the threshold,
      inject with a clear provenance marker
- [ ] Token budget for injected memories, so recall cannot crowd out the
      conversation
- [ ] `whycode memory list|search|delete|clear`
- [ ] `--no-memory` flag and `[memory] enabled` config key
- [ ] Measure with the perf harness: RSS with memory on versus off, and recall latency

## Acceptance criteria

- [ ] Embedding runs with the network disabled
- [ ] A fact written with `/remember` in one session is recalled in a new
      session when the topic recurs, and is not recalled on an unrelated topic
- [ ] Recall adds under 100 ms to a turn on the benchmarks reference machine
- [ ] RSS delta from enabling memory is measured and recorded in
      `docs/benchmarks.md`
- [ ] Injected memories are visibly marked in the prompt, so a user reading a
      transcript can tell what came from memory
- [ ] Injected memories never exceed their token budget
- [ ] `whycode memory delete` removes an entry and it is not recalled again
- [ ] With memory disabled, RSS and startup match the pre-phase baseline
- [ ] The model file's integrity is verified if downloaded rather than bundled

## Risks

- **Binary size.** A bundled MiniLM roughly triples the download. Downloading
  on first use trades that for a network dependency and a supply-chain surface
  that then needs checksum pinning.
- **Recall precision.** Wrong memories injected confidently are worse than no
  memory. Start with a high similarity threshold and small k; loosen only with
  evidence.
- **Silent context growth.** Memories consume prompt tokens the user did not
  ask for. The token budget and the provenance marker are what keep this
  honest.
- **Scope creep into code search.** Embedding memories and embedding the
  codebase look similar and are not. Keep them separate.

## Reference

`jcode/crates/jcode-embedding` (572 lines, `all-MiniLM-L6-v2` via
`tract-onnx` + `tokenizers`), `jcode-memory-types` (1,700 lines),
`jcode/scripts/test_memory.py`, `jcode_memory_snapshot.py`.
