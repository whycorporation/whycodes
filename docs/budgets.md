# Quality budgets

CI enforces formatting, clippy and tests. Those catch a change that is wrong.
They do not catch slow drift: an `unwrap()` added under pressure, an error
quietly discarded, a dependency edge that appears because one file needed one
import.

Budgets are counted properties with a recorded ceiling. They exist to make that
drift visible, not to be at zero.

## The budgets

| Check | Counts | Budget file |
|---|---|---|
| `check_panic_budget.py` | `unwrap()`, `expect()`, `panic!()`, `todo!()`, `unimplemented!()` in non-test code | `scripts/panic_budget.json` |
| `check_swallowed_error_budget.py` | `let _ = call(…)`, `Err(_) =>`, `.ok();` in non-test code | `scripts/swallowed_error_budget.json` |
| `check_memory_budget.py` | Bounds of in-memory caches and retained buffers (entries, bytes, live sessions) | `scripts/memory_budget.json` |
| `check_dependency_boundaries.py` | `whycodes-*` edges between workspace crates | `scripts/dependency_boundaries.json` |
| `check_sdk_protocol.py` | Protocol v1 tags (`SdkEvent` / `ErrorCode`) vs TypeScript `KNOWN_EVS` | (no JSON; fails on drift) |
| `check_tracked_secrets.py` | Tracked scratch dirs, private keys, live-looking PATs | (no JSON; fails on a hit) |

Run them the way CI does:

```bash
python scripts/check_panic_budget.py
python scripts/check_swallowed_error_budget.py
python scripts/check_memory_budget.py
python scripts/check_dependency_boundaries.py
python scripts/check_sdk_protocol.py
python scripts/check_tracked_secrets.py
```

Each prints the offending file and line, not just a count — a check that only
says "47 > 46" tells you nothing about which one to look at.

## The ratchet

Going **below** a budget passes, and the script says which crates have slack.
Lowering the number is then a one-line follow-up that locks in the improvement.

Going **above** fails. The only way past that is to edit the JSON, which appears
in the diff and has to be justified in the commit message. That is the whole
mechanism: the number can improve quietly and can only worsen deliberately.

## Raising a budget

Sometimes correct. A new crate arrives with legitimate `expect()` calls at a
startup boundary where failure genuinely is unrecoverable; a dependency edge is
the right design. Raise it, and say in the commit message what the additions are
and why they are not the thing the budget is meant to prevent.

Do not raise a budget to land unrelated work. That is what the ratchet is for.

## Memory

Idle memory is measured ([benchmarks.md](benchmarks.md#multi-session-pss)), but a
measurement only says that memory grew, after the fact. What makes it grow over
a long session is a cache or a retained buffer whose bound got raised, or a new
one with no bound at all. The memory budget makes those bounds a reviewed
decision.

[`scripts/memory_budget.json`](../scripts/memory_budget.json) lists each bound
as the Rust `const` that holds it: file, constant name, recorded value, and a
`kind` (`cache`, `buffer`, `count`, `transcript`, `index`, `allocator`) with a
one-line `what`. Today that covers the LLM response cache, the tool-definition
memo, the highlight and mermaid memos, the TUI's painted-line window, input
history, thinking and tool-output caps, live-session and background-job counts,
the background-job output tail, transcript and context-file caps, the workspace
index limits, and the heap-trim thresholds.

`check_memory_budget.py` reads each constant (a literal such as `64 * 1024` or
`80_000`) and fails when:

- the value is **above** the recorded one;
- the constant or its file is gone, or it is defined more than once in the file;
- the value is no longer a plain number (it now refers to another constant).

A value below the recorded one passes and is listed as slack, as with the other
budgets.

**Raising a bound.** Change the constant and its `value` in the JSON in the same
commit, and say in the message what the extra memory buys (a measured hit rate,
a user-visible truncation it removes). If the bound moved to another file or was
renamed, update `file` / `const`; if the cache was deleted, delete the entry.

**Adding a cache.** The script cannot find a new cache on its own. Give it a
named `const` bound and add the entry to the JSON in the same change; a cache
with no bound is the thing this budget exists to prevent.

## What these numbers are not

They are crude on purpose.

- Not every `unwrap()` is a bug. One on a regex literal that is known to compile
  is fine. One in a request path is not. The count cannot tell them apart.
- Not every `Err(_)` hides a failure. Best-effort cleanup legitimately ignores
  errors.
- The dependency graph as recorded is not a claim that it is the right graph. It
  is a claim that changing it should be deliberate.

A budget that nobody ever lowers is bureaucracy. Pair a seeding commit with an
actual reduction so the ratchet starts moving.

## Why this exists

The motivating case is in this repository's history. `cmd_stats` swallowed a
database error that `cmd_session` propagated:

```rust
let db = match open_db() {
    Ok(d) => d,
    Err(_) => { println!("No statistics database found."); return Ok(()); }
};
```

A macOS CI failure took three rounds to diagnose because of that asymmetry: one
command reported the fault and the other reported "no database found", which
reads as a fresh install rather than as a locked file. The fix distinguishes
*missing* from *broken*, and the swallowed-error budget dropped by one.

## Deliberately not budgeted

- **Warnings.** `-D warnings` already means zero; a budget adds nothing.
- **Startup time and measured memory.** Harness exists ([benchmarks.md](benchmarks.md));
  CI ceilings shipped. The bounds that drive memory growth are budgeted
  ([Memory](#memory)); the measured PSS is not.
- **Test size, wildcard re-exports.** Reasonable ideas, low value at this size.
- **Binary size.** Needs release artifacts; belongs with packaging rather than here.
