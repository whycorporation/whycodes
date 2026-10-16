# <Feature> acceptance

> Status: Verified · Updated: YYYY-MM-DD · Commit: `<sha>` · Binary: `<target>` SHA-256 `<hex>`

One paragraph: what shipped, which user-visible promise it makes, and which
page documents the promise (link to `../guide.md#…` or an internals page).

Copy this file to `docs/acceptance/<feature>.md` when a feature lands whose
behaviour cannot be fully pinned by unit tests: anything that touches a real
provider, a real terminal, the OS sandbox, the network, or another process.
Fill every row. A row with no observation stays in the table as **not
proven**; deleting it is not allowed.

## Requirement-to-observation ledger

| # | Requirement (the promise) | Concrete check (test name, command, or manual step) | Observed | Not proven / boundary |
|---|---|---|---|---|
| 1 | | `cargo test -p whycodes-<crate> <test> -- --exact` | | |
| 2 | | `whycodes … --format json` on `<fixture>` | | |
| 3 | | Manual: `<terminal>`, `<OS>`, steps 1–3 | | |

Rules for the table:

- **Requirement** is one sentence a user could read in the guide.
- **Concrete check** is reproducible by someone else: a test name with
  `--exact`, a command with its arguments, or numbered manual steps naming
  the terminal, OS and version.
- **Observed** is what actually happened, with the number or the exact
  output line, not "works".
- **Not proven** names the boundary: the provider, OS, terminal, size or
  concurrency that this check did not cover.

## Evidence split

| Kind | Where | Notes |
|---|---|---|
| Synthetic (scripted provider, fixtures, `WHYCODES_BENCH`) | | |
| Real provider / real terminal | | Provider, model, date, cost in tokens |

Keep the two apart. A synthetic pass does not count as a real-provider pass.

## Measurements (when the promise is a number)

| Metric | Method | Samples | Median | Machine |
|---|---|---|---|---|
| | | n = | | |

Report medians with the raw sample count. Do not divide numbers measured
on different machines or different builds.

## Remaining limits

- What was not exercised and why.
- Known follow-ups, each with an issue link.

## Related

- The feature's user doc: `../guide.md#…`
- The internals page: `../internals/….md`
- The PR: `#…`
