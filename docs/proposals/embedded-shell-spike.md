# Embedded shell spike

> Status: Design · Updated: 2026-10-16

## Problem

README.md promises "**One native binary.** No required runtime dependencies."
The `bash` tool does not keep that promise on Windows. `crates/tools/src/shell.rs`
hands every command to `whycodes_sandbox::run_timeout`, and
`crates/sandbox/src/host.rs::resolve_windows_host_shell` picks the interpreter:

1. `WHYCODES_SHELL` if set (and not the WSL stub);
2. Git for Windows' `bash.exe` from `PATH` or the well-known install roots;
3. `cmd.exe /C`, with a per-session warning that "Unix-style commands may fail".

`docs/guide.md` ("Shell OS sandbox") documents this and says Git for Windows is
"recommended ... not required". In practice the model emits `ls`, `grep -rn`,
`cat`, `sed -i`, `head`, `wc -l`, pipes, `&&` and `$VAR` on every platform, so a
Windows host without Git Bash degrades to a stream of "is not recognized"
failures. The system prompt (`crates/agent/src/agent/mod.rs`, "The `bash` tool
runs commands in {shell}") tells the model it is in `cmd.exe`, but models do not
reliably switch dialect.

A second, independent motivation: `crates/command-risk` classifies commands with
its own splitter (`tokenize.rs`, "This is not a shell parser"). A real parser
would let the classifier see nested command substitutions, heredoc bodies and
redirect targets as structure rather than as flat words.

This spike asks whether to embed a Rust shell plus coreutils in `whycodes` so
that both problems go away, and at what cost.

## Candidates

All figures below were fetched on 2026-10-16 from crates.io, docs.rs and GitHub
and are untrusted third-party data; verify before relying on any single number.
The workspace is edition 2024 with `rust-version = "1.91"`, so an MSRV of 1.88
fits and an MSRV of 1.96 does not.

| Crate | Role | License | Latest / date | MSRV · edition | Windows | Library shape |
|---|---|---|---|---|---|---|
| `brush-parser` 0.4.0 | bash/POSIX tokenizer + parser | MIT | 2026-05-03 | 1.88 · 2024 | pure Rust, no OS deps | `tokenize_str`, `parse_tokens`, `ast::*`, `word::parse` |
| `brush-core` 0.5.0 | shell runtime (expansion, execution, jobs) | MIT | 2026-05-03 | 1.88 · 2024 | "Preview" tier | `Shell::builder()`, `run_string` (async), `register_builtin`, `stdout()/stderr()` capture |
| `brush-shell` 0.4.0 | the `brush` binary (2.3k stars, ~1.3k commits) | MIT | 2026-05-03 | 1.88 · 2024 | Preview; no prebuilt Windows binaries yet (#1239) | binary only |
| `brush-coreutils-builtins` 0.1.0 | uutils wrapped as brush builtins | MIT | 2026-05-03 | 1.88 | same as uutils | `bundled_commands()` registry of `uumain`-style fns |
| `coreutils` / `uu_*` / `uucore` 0.12.0 | coreutils as per-utility crates (24.2k stars) | MIT | 2026-09-17 | 1.88 · 2024 | `windows` feature = `feat_Tier1 + stdbuf`; Unix-only: chmod, chown, chgrp, chroot, id, stat, stty, mkfifo, who, uptime | each `uu_X` exports `uumain(args) -> i32` and `uu_app() -> clap::Command` |
| `findutils` 0.10.0 | `find`, `xargs` | MIT | 2026-08-03 | unset | yes | binaries; library surface not documented |
| `sed` 0.2.0 (uutils/sed) | `sed` | MIT | 2026-08-29 | 1.88 | yes | binary-first crate |
| `diffutils` 0.5.0 | `diff` | MIT OR Apache-2.0 | 2026-01-02 | unset | yes | binary-first crate |
| `nu-engine` 0.116.1 | Nushell evaluator | MIT | 2026-10-04 | 1.96.1 | yes | not POSIX; own language |
| `yash-syntax` 0.25.0 | POSIX sh parser | GPL-3.0-or-later | 2026-09-06 | 1.96 | pure Rust | parser only |
| `conch-parser` 0.1.1 | POSIX sh parser | MIT/Apache-2.0 | 2019-05-15 | unset | pure Rust | parser only |

Notes:

- **brush** is the only candidate that is both a bash-compatible runtime and
  designed to be embedded ("Start here to embed" on `brush-core`). Its README
  reports a compatibility suite of 2,500+ cases diffed against real bash, and a
  known-gaps list (`select`, `wait -n`, `disown`, unreliable `$!`, signal traps
  "in progress"). `brush-core` pulls `tokio`, `clap`, `fancy-regex`, `chrono`,
  `rand`, `cached`; on Unix also `nix`, `command-fds`, `terminfo`, `uzers`. We
  already ship `tokio`, `clap`, `chrono` and `regex`, so the marginal dependency
  set is moderate. `run_string` is `async`, which fits our Tokio runtime.
- **brush-coreutils-builtins** is the exact bridge this spike would otherwise
  have to write: it ports `uucore::bin!` so each utility returns an `i32`
  instead of calling `exit`, receives argv from the caller, and is looked up by
  name. Its documented design detail matters for us: the host also registers
  per-name shims that "re-enter the binary as an external process" so that
  redirections, pipes and process-group state work. Caveat: 0.1.0 pins
  `uu_* ^0.8.0` while uutils is at 0.12.0, so a straight dependency would drag a
  second, older `uucore` into the build.
- **uutils** is a multicall ("BusyBox-type") binary by default; `src/bin/coreutils.rs`
  dispatches on `argv[0]` or the first argument via a `build.rs`-generated map of
  `name -> (uumain, uu_app)`. Reported stripped sizes for the full multicall
  binary range from ~12 MB (`panic=abort`, fat LTO) to ~17 MB (thin LTO); the
  0.12.0 Windows release zips are 4.6–4.9 MB compressed. A coding agent needs
  roughly a fifth of the utilities, so the realistic increment is far smaller
  (estimate below).
- **`grep` has no uutils crate on crates.io.** Microsoft's preview "Coreutils
  for Windows" bundles a fork of a grep implementation, but as a binary. We
  already link `grep-searcher` and `grep-regex` in `whycodes-tools`, so a
  `grep` builtin with the common flags (`-r -n -i -l -E -F -v -c -A/-B/-C`) is a
  small adapter, not a new dependency.
- **`which`** is neither coreutils nor findutils; it is a trivial PATH walk and
  would be written in-house.
- **nushell** is excluded: it is not a POSIX dialect, so models' emitted
  commands would fail in a different way. **yash-syntax** is excluded by
  licence (GPL-3.0-or-later cannot be linked into an MIT product) and MSRV
  (1.96). **conch-parser** has had no release since 2019.

Utilities a coding agent needs, grouped by source: `ls cat head tail wc sort
uniq cut tr mkdir rm cp mv echo pwd env tee touch printf true false test`
(uutils, all in `feat_common_core` and on Windows); `find xargs` (findutils);
`sed` (uutils/sed); `diff` (diffutils); `grep` and `which` (in-house). `chmod`
and `chown` are Unix-only in uutils and would be no-ops or errors on Windows.

## Windows reality

What brush documents and what its tracker shows for Windows today:

- Tier: "Preview. Windows receives build and static checks in CI along with a
  small brush-specific test suite; prebuilt binaries are on the way. Parts of
  the shell are still missing or limited but it's functional for basic usage,
  particularly when paired with Microsoft's build of coreutils for Windows."
  The full compatibility suite runs only on Linux and macOS aarch64.
- There is no `fork`: brush spawns externals through the standard process API
  and runs builtins in-process, so the model of "shell = interpreter + spawn"
  holds on Windows. Process substitution and fd passing depend on
  `command-fds` (`cfg(unix)`), so `<(cmd)` / `>(cmd)` should be assumed absent
  on Windows. Job control (`&`, `fg`, `bg`) is documented for Unix; nothing is
  claimed for Windows, and we do not need it because the agent runs commands
  non-interactively with a timeout.
- Open Windows issues (as reported, state at fetch time):
  `#1359` pipelines hang above 4096 bytes through a pipe (open, untriaged, no
  maintainer reply); `#1364` Ctrl-C does not interrupt a running external;
  `#1317` PATHEXT-resolved commands such as `npx` spawn unresolved (os error
  193); `#1251` relative external paths fail after `cd`; `#1142` MSYS-style
  `/c/Users/...` paths are not resolved (maintainer: "very open to input");
  `#1372` CR characters produce poor errors; `#1452` `name.cmd` on PATH could
  not be run as `name` (closed, fixed 2026-10).
- Path separators: brush uses `normalize-path`; drive-letter and backslash paths
  work as native Windows paths. The MSYS `/c/` convention is a Git Bash feature
  that an embedded shell will not provide unless we add a translation layer.
- Name collisions: Windows ships `C:\Windows\System32\find.exe` (DOS find) and
  `sort.exe`. An embedded shell must resolve bundled builtins before `PATH`, or
  `find . -name '*.rs'` silently runs the wrong program.
- `#1359` is the one item that is a blocker for an agent: `cargo test 2>&1 |
  tail -50` moves more than 4 KiB through a pipe on every call.

Conclusion: brush on Windows is credible as a *fallback* behind a real Git Bash,
not as a replacement for it today. The pipe-hang issue must be closed (or
reproduced and fixed by us) before it ships to users.

## Risk classifier on an AST

`crates/command-risk` is dependency-free and `assess.rs` works on
`tokenize::Segment { words: Vec<Word { text, dynamic }> }`. `brush-parser`'s AST
is `Program -> CompoundList -> AndOrList -> Pipeline -> Command`, where
`Command` is `Simple(SimpleCommand) | Compound(CompoundCommand, Option<RedirectList>)
| Function(FunctionDefinition) | ExtendedTest(..)`, `CompoundCommand` has
`BraceGroup | Subshell | ForClause | CaseClause | IfClause | WhileClause |
UntilClause | Arithmetic | ArithmeticForClause | Coprocess`, and `SimpleCommand`
is `{ prefix, word_or_name, suffix }` with items
`IoRedirect(IoRedirect) | Word(Word) | AssignmentWord(Assignment, Word) |
ProcessSubstitution(kind, SubshellCommand)`. `IoRedirect` is
`File(Option<fd>, kind, target) | HereDocument(..) | HereString(..) |
OutputAndError(Word, append: bool)`. `Word.value` is the raw unexpanded text;
`word::parse` splits it into pieces (quoted text, parameter expansion, command
substitution, tilde, arithmetic).

What the classifier would gain:

- **Nested command substitution becomes visible.** Today `tokenize` consumes
  `$(...)` and sets `dynamic`; the inner text is dropped. `rm -rf $(cat list)`
  escalates correctly, but `echo $(rm -rf ~)` is `Safe`: the segment's command
  is `echo`, `has_dynamic()` only escalates a base verdict that is already
  `Caution` or worse, and `mentions_destructive_command` only runs when the
  line is `malformed`. With the AST the substitution body is a `Program` and
  is assessed recursively, so it would be `Catastrophic`. Same for backticks
  and for `"$(...)"` inside double quotes.
- **Heredoc bodies are data, not commands.** `cat <<EOF ... rm -rf ~ ... EOF`
  is split on `\n` by the tokenizer and the body line is classified as a
  command (a false positive in the safe direction, but a prompt the user
  learns to ignore). `IoRedirect::HereDocument` carries the body separately.
- **Redirect targets are typed.** `IoRedirect::File(fd, kind, target)` and
  `OutputAndError(word, append)` replace the `REDIRECTS` table and the
  "peek the next word" logic in `assess_redirects`; `>|`, `&>`, `2>&1`, `<>`
  and fd-numbered forms come for free, as does distinguishing `>>` append.
- **Quoting is exact.** `tokenize` already strips quotes, but `word::parse`
  yields which pieces were single-quoted (never expanded), which were
  `${VAR}` expansions, and which were tildes, so `paths::expand` can be fed a
  precise `"$HOME"`/`${HOME}`/`~user` decision instead of string matching.
- **Assignments are structure.** `AssignmentWord` lets a later phase do
  single-pass constant propagation: `d=~; rm -rf "$d"` is `Destructive`
  (unresolved variable) today and could become `Catastrophic`.
- **Compound bodies are walked, not guessed.** The `SHELL_KEYWORDS` skip list
  and the "`(` and `)` are separators" rule are replaced by a real walk of
  `IfClause`/`ForClause`/`CaseClause`/`Subshell`/`FunctionDefinition` bodies.
  Function definitions can be recorded so `f(){ rm -rf ~; }; f` is assessed at
  the call, not only at the definition.
- **Process substitution** is a typed `ProcessSubstitution(kind, SubshellCommand)`
  whose body is again a `Program` to assess, instead of the byte scan in
  `has_process_substitution`.

What it would lose or put at risk:

- **Dialect coverage.** brush parses bash. The tokenizer deliberately handles
  Zsh `=(cmd)`, and `WHYCODES_SHELL` may be `pwsh.exe` or `cmd.exe`, whose
  syntax brush rejects. Any parse error must fall back to the current
  tokenizer, and the current `malformed` escalation must stay.
- **The absolute tier must stay parser-independent.** `paths.rs` is the
  guarantee that "a command that defeats the parser must still not be able to
  delete a home directory". Phase 2 must lower the AST to the existing
  `Segment` shape and keep `assess_segment`/`paths::classify` unchanged, so the
  AST is an additional front-end, not a replacement.
- **A dependency in a zero-dependency crate.** `command-risk` has no
  dependencies, a Criterion bench (`benches/assess.rs`) with a CI ceiling, and
  a line-coverage floor. `brush-parser` adds a parser (and `winnow`/`miette`
  behind features) to every crate that links `command-risk`
  (`agent`, `subagent`); the panic-budget ratchet must account for parser
  panics, and parse time per command must be measured against the bench.
- **Two sources of truth if Phase 1 ships without Phase 2.** If the shell that
  runs the command is brush but the classifier still tokenizes, disagreements
  on quoting are possible. Using the same parser for both closes that gap,
  which is a point in favour of doing both phases.

## Sandbox interaction

Today `crates/sandbox` has two backends: `bwrap.rs` builds
`bwrap --ro-bind / / --bind <cwd> <cwd> ... -- bash -c <cmd>`; `host.rs` builds
`bash -c <cmd>` (Unix) or the Windows invocation above. `policy.rs` spawns the
`PreparedCommand`, captures output, applies the timeout and kills the process
group (`kill_pid_group`). `shell.rs` only wraps that in `spawn_blocking`.

An embedded shell changes different things depending on where it runs:

- **In the agent process (builtins in-process):** on Linux this *breaks*
  the sandbox: the agent process is outside the bwrap namespace, so an
  in-process `rm` would bypass the read-only root. It also removes the
  timeout/kill story (there is no child to kill), and a panic in any `uu_*`
  crate aborts the TUI because the release profile is `panic = "abort"`.
  Not acceptable on any platform.
- **As a child process, re-executing our own binary** (`whycodes __shell -c
  <cmd>`): bwrap keeps working with a one-line change (`bash` becomes
  `std::env::current_exe()` inside the `--` argv, and the binary is already
  visible under the read-only root bind); `kill_pid_group`, `--die-with-parent`,
  the timeout and the `background` job path are unchanged; panics are isolated.
  This is also what `brush-coreutils-builtins` documents as the pattern that
  makes pipes and redirections work for bundled utilities. The cost is one
  process spawn per command, which we already pay for `bash`.
- **On Windows there is no OS sandbox today**, so the only change is which
  interpreter the child runs. The risk classifier remains the single gate, which
  is why Phase 2 is worth doing alongside Phase 1.

So: the shell must always run as a child of the agent process. `crates/sandbox`
does not need to depend on the new shell crate at all; it only needs to know
the program path (`current_exe`) and the argv prefix. Only `whycodes-cli` links
the shell crate, which keeps the dependency graph one-way
(`scripts/dependency_boundaries.json` gains a single `cli -> shell` edge).

## Decision table

Baseline: `target/release/whycodes.exe` is 18.5 MB (LTO, `codegen-units=1`,
`strip`, `panic=abort`). Size increments are estimates derived from the
reported sizes above (brush binary 2.5–2.7 MB compressed; full uutils multicall
12–17 MB stripped for ~90 utilities) scaled to the ~22 utilities an agent needs,
plus `sed`, `find`/`xargs` and an in-house `grep`. Measure before trusting.

| | A. Keep external shell, document Git Bash | B. Embed as Windows fallback | C. Embed as default everywhere |
|---|---|---|---|
| Binary size | +0 | +6–9 MB on Windows targets only (`cfg(windows)` deps); +0 elsewhere | +6–9 MB on every target |
| Risk | Low. Status quo; Windows without Git Bash stays degraded | Medium. brush Windows is Preview; `#1359` pipe hang, PATHEXT and `/c/` gaps; collisions with DOS `find`/`sort` | High. Linux/macOS users lose real bash (`.bashrc`, aliases, bash 5 features, signal traps); every bwrap path changes; every tool output changes flavour (GNU-style on macOS) |
| Effort | 1–2 person-days: guide wording, first-run hint, startup diagnostic | 12–20 person-days: shell crate, hidden subcommand, host.rs fallback, grep/which builtins, Windows test matrix, size CI | 25–40 person-days: B plus bwrap re-exec, Unix parity testing against the brush suite, migration of `host_shell_name`/prompt wording, opt-out config |
| User-visible change | None. Warning text improves | Only on Windows hosts without Git Bash: Unix-style commands work; `whycodes` reports the shell as its own bash-compatible shell | Everywhere: one shell dialect on all platforms; some scripts that relied on bash extras break; outputs become byte-identical across OSes |
| Reversibility | n/a | Feature flag `embedded-shell`; fallback order keeps Git Bash first | Hard: users will have scripts tuned to the embedded dialect |

Classifier-on-AST (Phase 2) is orthogonal to the three options: it is useful
even under A, because `echo $(rm -rf ~)` is a real gap today.

## Recommendation and phased plan

Recommended: **A now, B next, C not now.** Ship the documentation fix
immediately; build B behind a Cargo feature that is on by default only for
Windows targets, ordered *after* Git Bash and *before* `cmd.exe`; run the
classifier AST work as its own phase with a tokenizer fallback; revisit C only
after brush moves Windows to its "Supported" tier and the embedded path has
soaked on Windows for two releases.

Phase 0 (A, this sprint): make the current behaviour honest.
- `docs/guide.md` "Shell OS sandbox": say plainly that Unix-style commands need
  Git for Windows, and that `cmd.exe` is a degraded mode.
- README "One native binary" bullet: add "the `bash` tool prefers Git for
  Windows on Windows; without it commands run in `cmd.exe`".
- `crates/sandbox/src/host.rs`: keep the warning but surface it once at startup
  (the TUI status line) rather than only on the first command.

Phase 1 (B): embedded shell as a child-process fallback.
- New foundation crate `crates/shell` (`whycodes-shell`), depends on
  `brush-core`, `brush-coreutils-builtins` (or direct `uu_*` 0.12 if the
  version pin is not lifted), `sed`, `findutils`, plus `grep-searcher`/`grep-regex`
  for an in-house `grep`. Exposes `pub fn run_dash_c(command: &str, cwd: &Path)
  -> i32` that builds `brush_core::Shell::builder()`, registers builtins so they
  win over `PATH`, runs `run_string`, and flushes captured stdout/stderr.
- Hidden subcommand `whycodes __shell -c <cmd>` in `crates/cli/src/main.rs`,
  dispatched before config/storage load so it starts fast.
- `resolve_windows_host_shell`: new candidate between `native_bash` and
  `comspec`: `program = current_exe`, `args_prefix = ["__shell", "-c"]`,
  no warning. `host_shell_name()` returns a stable name the system prompt can
  use ("whycodes shell (bash-compatible)").
- Verify: the `bash` tool integration tests in `crates/tools/src/shell_tests.rs`
  pass on a Windows runner with Git Bash removed from `PATH`; a pipe test moves
  1 MiB through `cat | wc -c`; `find . -name` returns results in a tree.
- CI: a size check that fails if the Windows release binary grows by more than
  the agreed budget; `cargo deny` licence check for the new tree.

Phase 2 (classifier): `brush-parser` front-end for `command-risk`.
- `crates/command-risk/src/ast.rs` behind feature `parser`: parse, walk, lower
  each `SimpleCommand` to a `Segment`, assess substitution bodies recursively,
  skip heredoc bodies, map `IoRedirect` to the existing truncating-redirect
  rule. On `ParseError`, fall back to `tokenize` unchanged.
- Corpus additions in `assess.rs` tests: `echo $(rm -rf ~)` is `Catastrophic`;
  heredoc body containing `rm -rf ~` is `Safe`; `2>&1` is not a truncating
  redirect; `pwsh`-syntax input stays on the tokenizer path.
- Bench `crates/command-risk/benches/assess.rs` before/after; keep the ceiling.

Phase 3 (C, decision gate only): re-evaluate with data from Phase 1 telemetry
(fraction of Windows sessions on the embedded path, failure rate per command
family) and brush's Windows tier. Do not pre-commit.

First three concrete tasks:

1. `crates/sandbox/src/host.rs` + `crates/sandbox/src/tests.rs`: add the
   embedded candidate to `resolve_windows_host_shell` under
   `#[cfg(feature = "embedded-shell")]`, with unit tests for the three orderings
   (override > Git Bash > embedded > cmd). No new crate dependency here.
2. `crates/shell/src/lib.rs` (new, `whycodes-shell`) with `run_dash_c`, a
   `grep` builtin over `grep-searcher`, a `which` builtin, and a smoke test that
   runs `printf 'a\nb\n' | wc -l && echo $?`; `crates/cli/src/main.rs` hidden
   `__shell` subcommand; `scripts/dependency_boundaries.json` edge
   `cli -> shell`; `scripts/check_panic_budget.py` allowlist review.
3. `crates/command-risk/src/ast.rs` prototype + `Cargo.toml` optional
   `brush-parser`, lowering to `Segment`, with the four corpus tests above and a
   bench run recorded in the PR.

## Open questions

- `brush-coreutils-builtins` 0.1.0 pins `uu_* ^0.8.0` against uutils 0.12.0.
  Wait for a bump, vendor the 150-line adapter, or depend on `uu_*` directly?
- Should `/c/Users/...` (MSYS) paths be translated for the model's benefit?
  `paths.rs` already normalises `C:/`; `/c/` is neither Unix nor Windows there.
- PATH precedence: builtins first (predictable, hides a user's own `sort.exe`)
  or `PATH` first (bash-like, but hits DOS `find.exe`)? Proposed: builtins
  first for the bundled set only.
- `#1359` (pipe hang above 4 KiB on Windows) must be reproduced in our harness
  before Phase 1 ships; if it is in brush's Windows pipe implementation we may
  need to carry a patch.
- Coverage: the workspace floor is 98.5 %. A crate that is mostly glue over
  third-party `uumain`s is hard to cover line-by-line; exclude it in
  `scripts/coverage.sh` or keep the glue thin enough to cover?
- What does the system prompt call the shell? Models adapt their dialect to
  the name; "bash-compatible" should be in the string.
- Secret redaction (#278) restores placeholders in tool arguments before the
  command runs; confirm that path is upstream of the new interpreter and
  unaffected.
- Output encoding on Windows: embedded utilities emit UTF-8 with `\n`; Git Bash
  does too, `cmd.exe` does not. Any consumer that assumed CRLF?
- Is a per-platform feature default (`embedded-shell` on for Windows only)
  acceptable for the installer scripts and the release workspace build in CI?

## Sources

- brush repository and README (tiers, crates, compatibility claims):
  https://github.com/reubeno/brush
- brush compatibility reference: https://github.com/reubeno/brush/blob/main/docs/reference/compatibility.md
- brush Windows issues: https://github.com/reubeno/brush/issues/1359 ,
  https://github.com/reubeno/brush/issues/1142 ,
  https://github.com/reubeno/brush/issues/1296 ,
  https://github.com/reubeno/brush/issues?q=is%3Aissue+windows
- brush releases (asset sizes): https://github.com/reubeno/brush/releases
- crates.io metadata: https://crates.io/crates/brush-core ,
  https://crates.io/crates/brush-parser , https://crates.io/crates/brush-shell ,
  https://crates.io/crates/brush-coreutils-builtins ,
  https://crates.io/crates/coreutils , https://crates.io/crates/uucore ,
  https://crates.io/crates/uu_ls , https://crates.io/crates/findutils ,
  https://crates.io/crates/sed , https://crates.io/crates/diffutils ,
  https://crates.io/crates/nu-engine , https://crates.io/crates/yash-syntax ,
  https://crates.io/crates/conch-parser
- brush-core API: https://docs.rs/brush-core/latest/brush_core/struct.Shell.html ,
  https://docs.rs/brush-core/latest/brush_core/builtins/index.html ,
  https://docs.rs/brush-core/latest/brush_core/builtins/trait.Command.html ,
  https://docs.rs/brush-core/latest/brush_core/commands/struct.ExecutionContext.html ,
  https://crates.io/api/v1/crates/brush-core/0.5.0/dependencies
- brush-coreutils-builtins design: https://docs.rs/brush-coreutils-builtins/latest/brush_coreutils_builtins/
- brush-parser AST: https://docs.rs/brush-parser/latest/brush_parser/ast/index.html ,
  https://docs.rs/brush-parser/latest/brush_parser/ast/enum.Command.html ,
  https://docs.rs/brush-parser/latest/brush_parser/ast/enum.CompoundCommand.html ,
  https://docs.rs/brush-parser/latest/brush_parser/ast/enum.CommandPrefixOrSuffixItem.html ,
  https://docs.rs/brush-parser/latest/brush_parser/ast/enum.IoRedirect.html ,
  https://docs.rs/brush-parser/latest/brush_parser/word/index.html
- uutils coreutils: https://github.com/uutils/coreutils ,
  https://raw.githubusercontent.com/uutils/coreutils/main/Cargo.toml ,
  https://raw.githubusercontent.com/uutils/coreutils/main/src/bin/coreutils.rs ,
  https://docs.rs/uucore/latest/uucore/ , https://github.com/uutils/coreutils/releases
- uutils multicall size reports: https://discourse.ubuntu.com/t/migration-to-rust-coreutils-in-25-10/59708/3 ,
  https://bugs.archlinux.org/task/74413 ,
  https://pkgs.alpinelinux.org/package/v3.23/community/x86/uutils
- Microsoft "Coreutils for Windows" (preview; bundles coreutils, findutils, grep):
  https://github.com/microsoft/coreutils , https://www.phoronix.com/news/MS-Coreutils-For-Windows
- In-repo: `crates/tools/src/shell.rs`, `crates/sandbox/src/{host.rs,bwrap.rs,policy.rs}`,
  `crates/command-risk/src/{assess.rs,tokenize.rs,paths.rs}`,
  `crates/agent/src/agent/mod.rs` (system prompt shell name), `docs/guide.md`
  ("Shell OS sandbox"), `README.md` ("One native binary").
