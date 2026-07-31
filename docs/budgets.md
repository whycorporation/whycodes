# Quality budgets

CI enforces formatting, clippy and tests. Those catch a change that is wrong.
They do not catch slow drift: an `unwrap()` added under pressure, an error
quietly discarded, a dependency edge that appears because one file needed one
import.

Budgets are counted properties with a recorded ceiling. They exist to make that
drift visible, not to be at zero.

## The three budgets

| Check | Counts | Budget file |
|---|---|---|
| `check_panic_budget.py` | `unwrap()`, `expect()`, `panic!()`, `todo!()`, `unimplemented!()` in non-test code | `scripts/panic_budget.json` |
| `check_swallowed_error_budget.py` | `let _ = call(…)`, `Err(_) =>`, `.ok();` in non-test code | `scripts/swallowed_error_budget.json` |
| `check_dependency_boundaries.py` | `whycode-*` edges between workspace crates | `scripts/dependency_boundaries.json` |

Run them the way CI does:

```bash
python scripts/check_panic_budget.py
python scripts/check_swallowed_error_budget.py
python scripts/check_dependency_boundaries.py
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
- **Startup time and memory.** These need the harness from
  [5.md](5.md) before a ceiling means anything.
- **Test size, wildcard re-exports.** Reasonable ideas, low value at this size.
- **Binary size.** Needs release artifacts, so it belongs with the release
  workflow in [2.md](2.md) rather than here.
