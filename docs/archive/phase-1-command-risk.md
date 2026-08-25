# Phase 1 — Shell command risk classification

**Status:** done (2026-07-31) · **Depends on:** nothing · **Blocks:** nothing

## Problem

`ShellTool` executes whatever command string the model produces. The only gate
is the `[permission]` map, which is a per-tool-name decision made before the
command is known:

```toml
[permission]
bash = "allow"   # every command runs, unconditionally
bash = "ask"     # every command prompts, including `ls`
```

Neither setting is usable. `allow` means a model that emits `rm -rf ~` is
obeyed. `ask` prompts on every `cargo build` until the user turns it off, which
converges on `allow`. jcode hit exactly this: their issue #604 records a user
losing their home directory, and their `jcode-command-risk` crate exists as the
response.

There is a second problem specific to whycodes: when stdin is not a terminal the
prompter auto-denies, so in CI or a pipe `ask` silently degrades to `deny`. That
is safe but makes `ask` useless for automation.

## Goal

Classify a command by what it could destroy before running it, so `allow` can
stay on for the common case while destructive commands still stop.

## Design

Adopt jcode's two properties, which are the parts worth borrowing:

1. **Classify by blast radius, not command name.** A denylist of `rm -rf`
   misses `find -delete`, `shred`, `truncate`, `dd`, and `> file`. Ask what the
   command would destroy and whether it is recoverable.
2. **Bias toward recall.** A false positive costs one confirmation. A false
   negative costs a home directory. Escalate when parsing is ambiguous.

Levels as built:

| Level | Meaning | Behaviour at the default threshold |
|---|---|---|
| `Safe` | Read-only or confined to the project directory | runs |
| `Caution` | Writes or deletes inside the project | runs |
| `Destructive` | Reaches outside the project, or cannot be undone | prompts with the reason |
| `Catastrophic` | Targets `$HOME`, `/`, a device node, or the whole disk | refused, not promptable |

The catastrophic tier must be a small absolute path check that does not depend
on parsing the command correctly, because a determined `sh -c "$(printf ...)"`
defeats any static parser. This is defence in depth, not a sandbox, and the
docs must say so.

## Scope

In:

- New `crates/command-risk` with tokenizer, path resolution and classifier.
- Wire in ahead of the permission check.
- Config to tune where prompting starts.
- Document the model and its limits in the README permissions section.

Two deviations from this plan, both recorded in `status.md`:

- The gate sits in `Agent::execute_with_permission`, not `ShellTool::execute`.
  That is where the permission map already is, so "before the permission check"
  is expressible; `ShellTool` has no access to the prompter.
- The config key is `[security] bash_risk_threshold`, not under `[permission]`,
  because that map is typed `HashMap<String, PermissionAction>` and cannot hold
  a threshold string. Its default is `destructive`, not `caution`.

Out:

- Sandboxing, containers, seccomp. Different problem, much larger.
- Classifying non-shell tools. `edit`/`write` already have path handling.
- A model-based second stage. jcode has one; we do the deterministic stage
  first and only add a second if measurements justify it.

## Tasks

- [x] Create `crates/command-risk` and add it to the workspace
- [x] Tokenizer: split on `;`, `&&`, `||`, `|`, handle quoting and `$(...)`
- [x] Path resolution: expand `~`, resolve relative paths against the working
      directory, mark paths that escape it
- [x] Protected path set: `$HOME`, `/`, `/etc`, `/usr`, `/System`,
      `C:\Windows`, device nodes, and the drive root on Windows
- [x] Classifier covering at minimum: `rm`, `find -delete`, `shred`,
      `truncate`, `dd`, `mkfs`, `> file` redirection, `git reset --hard`,
      `git clean -fdx`, `chmod -R`, `chown -R`
- [x] Return a reason string with each non-`Safe` verdict, shown in the prompt
- [x] Wire into `ShellTool`, before the `PermissionAction` lookup
- [x] Config key and its default
- [x] README: document the tiers and state the limits honestly

## Acceptance criteria

- [x] A table-driven test covers every classifier rule, both the matching case
      and a near-miss that must stay `Safe`
- [x] `rm -rf ~`, `rm -rf /`, `rm -rf $HOME` and `rm -rf "$HOME"` all classify
      `Catastrophic` and are refused with `bash = "allow"` set
- [x] `ls`, `cargo build`, `git status`, `rg foo` classify `Safe` and run with
      no prompt under `allow`
- [x] A command writing outside the project directory classifies at least
      `Caution`
- [x] Classification is pure: no network, no filesystem writes, no model call
- [x] Tests pass on Linux, macOS and Windows — path handling is the part most
      likely to diverge
- [x] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --
      -D warnings`, `cargo test --workspace` all clean

## Risks

- **Over-blocking.** If `Caution` fires on ordinary work, users will raise the
  threshold to `off` and we are back to today. Keep `Safe` broad and measure
  against a corpus of real commands from a session transcript.
- **Windows path semantics.** Drive letters, UNC paths and case-insensitivity
  make "escapes the project directory" harder than on Unix. Budget for it.
- **False confidence.** The docs must not imply this is a sandbox.

## Reference

`jcode/crates/jcode-command-risk` — 1,659 lines, of which 772 are tests
(47%). Its `lib.rs` header documents the design and its limitations; worth
reading before starting. MIT licensed: any file derived from it keeps jcode's
copyright notice.
