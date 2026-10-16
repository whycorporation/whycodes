# Message voice

> Status: Implemented · Updated: 2026-10-16

How user-facing strings are written: status lines, notices, errors,
permission prompts, notifications, CLI output. One page so new strings sound
like the existing ones. This is not about the model's replies; it is about
what WhyCodes itself prints.

## The formula

Every notice follows the same three-part shape, in this order, and stops
when the next part has nothing to say:

1. **What happened.** One plain sentence in the past or present tense.
2. **What we did about it.** Only when WhyCodes acted on the user's behalf.
3. **How to change it.** The config key, flag or command, when one exists.

```
Prompt cache miss: 48.4k tokens sent uncached (last step read 46.0k from cache).
Context overflow — compacting and retrying this step…
Rate limited — retrying in 4s (attempt 2/5).
bwrap not found — running on the host. Set [security] sandbox_fallback = "deny" to fail instead.
```

Each line answers "why am I seeing this?" without the reader opening a log.

## Before and after

| Before | After | Why |
|---|---|---|
| `Gate armed: risk=destructive` | `This command deletes outside the project. Approve?` | Says what the user decides, not which internal path ran |
| `Compaction dispatched (reason=overflow)` | `Context overflow — compacting and retrying this step…` | Outcome first, mechanism second |
| `Hook failed` | `pre_tool hook refused bash (exit 2): "no network in CI"` | Names the hook, the tool, the exit code and the hook's own reason |
| `Error: 429` | `Rate limited — retrying in 4s (attempt 2/5).` | Tells the user whether to wait or act |
| `Sandbox fallback engaged` | `bwrap not found — running on the host.` | Plain words for the state the user is actually in |
| `Session persisted successfully` | *(nothing)* | Expected outcomes are silent |

## Rules

- **Lead with the outcome.** The first clause is what happened to the
  user's work: the turn, the tool, the file, the money.
- **Numbers carry units and a baseline.** `48.4k tokens`, `4s`, `attempt
  2/5`, `last step read 46.0k`. A bare number is a guess to the reader.
- **Name the knob exactly.** `[security] sandbox_fallback = "deny"`,
  `--no-auto-update`, `/fresh`. Never "see the docs" without the key.
- **One sentence per line in the status bar.** Two short sentences joined by
  an em dash are fine; a paragraph is not.
- **Silence for the expected.** Saved, loaded, connected, formatted: say
  nothing unless it failed or took unusually long.
- **Errors say who and what, never blame.** `pyright not on PATH` beats
  `LSP initialization failed`. Do not write "you must", "invalid", "illegal".
- **No internal vocabulary.** The words below describe implementation, not
  outcome. They stay in code and logs.

## Banned in user-facing strings

`gate`, `armed`, `dispatch`, `pipeline`, `orchestrator`, `handle`, `state
machine`, `invariant`, `fast path`, `hydrate`, `envelope`, `sink`, `hub`,
`claim` (say *another session is editing this file*), `worker-N` without the
agent's label, `catastrophic` (say *refused: this would damage the system*),
raw enum names (`PreTool`, `denied:headless` is fine only inside JSON output),
and any crate or module name.

Allowed because users configure them: `sandbox`, `hook`, `profile`,
`compact`, `steer`, `swarm`, `worktree`, `tool_search`.

## Where this applies

| Surface | Example | Note |
|---|---|---|
| TUI status line and toasts | `Rate limited — retrying in 4s (attempt 2/5).` | ≤ 80 columns; the TUI truncates beyond the width |
| Permission prompt | `bash wants to run \`rm -rf target\` (caution). Allow once / always / deny?` | Shows the command and the risk level in words |
| `--plain` and headless stderr | same strings, no colour | Keep them grep-able; do not change wording per surface |
| Desktop / Discord / Telegram notifications | `turn_done: Added retry tests (3 files)` | First line of the reply or the error, nothing else |
| CLI errors (`whycodes …` exits non-zero) | `No session matches "a1b2". Try \`whycodes session list\`.` | Outcome, then the next command |
| JSON output | stable snake_case keys, no prose | The prose rules do not apply to machine formats |

## Checklist for a new string

- [ ] First clause is the outcome.
- [ ] Any number has a unit and, where it helps, a baseline.
- [ ] The knob that changes the behaviour is named exactly, or there is none.
- [ ] No word from the banned list.
- [ ] The same wording is used on every surface that shows it.
- [ ] If the outcome is the expected one, the string was deleted.

## Related

- [User guide](guide.md) — the strings users actually see are quoted there;
  keep both in sync.
- [Know-how](knowhow.md) — some entries exist because a message hid the real
  cause.
