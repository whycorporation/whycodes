# Permission gate

> Status: Implemented · Updated: 2026-10-16

This page lists, in execution order, every check a tool call passes between
the model asking for it and the tool running: permission rules, shell risk
tiers, intent and path checks, the approval-mode overlay, the ask prompt and
its prompters, hooks, and the checks that live inside the tools themselves
(sandbox, file claims, HTTP domain policy). It is for contributors touching
`crates/agent/src/agent/gate.rs` or the policy crates; the user-facing rules
are in [`docs/guide.md`](../guide.md#permissions). Everything here is read from
the code; "not verified" marks what was not confirmed.

## How it works

### Gate order

`Agent::execute_with_permission` (`crates/agent/src/agent/gate.rs`) is called
once per tool call by `execute_tool_calls` (see
[turn-loop.md](turn-loop.md#10-tool-execution-scheduling)). The call carries
`ToolCall { id, name, arguments }`, the `Session`, the `ToolContext`, and the
turn's `IntentAssessment`. A single `risk_confirmed` flag records that the
user already approved this call with the command or path in hand, so later
`Ask` gates do not prompt twice.

0. **Before the gate.** Tools whose `action_for` is `Deny` are left out of
   the schema sent to the model (`ToolExecutor::get_definitions*`; exact
   filtering not verified). A matching speculative `read`
   (`take_speculative_read`) is returned **without** entering this function;
   speculation is off for `read` when any `pre_tool_transform`, `pre_tool`
   or `post_tool` hook matches it. The first thing inside the function is
   `transform_tool_call`: `pre_tool_transform` hooks
   (`whycodes_plugin::hooks::run_transform_hooks`) may replace the arguments
   with the JSON object they print, and every later step judges the
   rewritten call. Then `restore_tool_call` puts real values back where the
   model used secret placeholders (`whycodes_llm::Redactor::restore_json`),
   so the shell risk classifier and the tool see what will actually run.
   Workers do both before their headless gate.
1. **`question` short-circuit.** In `ApprovalMode::Auto` with open session
   todos or running background jobs the call is refused
   (`refuse_question_open_work`). Otherwise `should_prompt_questions` picks
   the real `QuestionPrompter` (always in `important` / `manual`; only for
   `important: true` questions in `auto`) or `AutoAnswerPrompter`. The
   permission map is never consulted for `question`.
2. **Shell risk classification** — `bash` / `shell`, and `schedule` with a
   non-empty `command`. `whycodes_command_risk::assess(command, working_dir)`
   returns a `RiskLevel` (`Safe` < `Caution` < `Destructive` <
   `Catastrophic`, `crates/command-risk/src/assess.rs`); `decide(assessment,
   risk_threshold)` (`crates/command-risk/src/lib.rs`) maps it to the
   outcomes below. The command is read by a recursive-descent shell parser
   (`parse.rs`) lowered to the tokenizer's `Segment`s; on a parse error
   (unbalanced quotes, PowerShell, a here-document without its terminator,
   nesting over 64) the tokenizer (`tokenize.rs`) is used as before.
   `paths.rs` and the per-segment rules in `assess.rs` are shared by both
   front-ends. Segments are assessed once with variables as written and once
   per value assigned earlier in the line (`DIR=/; rm -rf $DIR`, `for` items,
   `export`/`local`/`declare`); the worst reading wins, so a known value only
   adds risk. Here-document bodies are data unless the pipeline runs a shell
   (`bash <<EOF`, `cat <<EOF | sh`, `bash <<< '…'`), in which case they are
   assessed as a script. Redirections are typed: `2>&1`, `>&-` and reads
   write no file; `>&file` writes one. Comments are re-split by the tokenizer
   because `cmd.exe` has no `#` comments. Substitution bodies come from the
   raw scan plus the parser (which adds process-substitution bodies and
   bodies the scan cuts short at a quoted `)`).
   - `Refuse` — always for `Catastrophic` (home directory, system location,
     whole disk), whatever the threshold. Result:
     `Refused: <reason>. This command is classified catastrophic and cannot
     be approved. Run it yourself if you are certain.` No prompt, no hook.
   - `Confirm` — level at or above `security.bash_risk_threshold`
     (`caution` / `destructive` (default) / `off`): `ask_permission` with
     `Command:\n…\n\nRisk: <reason>`; denial → denied result; approval sets
     `risk_confirmed`.
   - `Allow` — continue.
3. **Shell-scoped rules** — `bash(git *)`, `shell(npm test)`,
   `Bash(cargo:*)` keys in the permission rules (`PermissionSet::action_for_shell`,
   `crates/core/src/types.rs`; longest matching pattern wins; a broad `Allow`
   on an interpreter such as `python *` is coerced to `Ask`). `Deny` → error
   `Permission denied for shell command by rule matching …`; `Allow` sets
   `risk_confirmed` only when step 2 decided `Allow`; `Ask` prompts unless
   already confirmed.
4. **Intent authorization** — `intent::authorize_tool`
   (`crates/agent/src/intent.rs`). Skipped when `session.intent_guidance =
   off` or the tool is not in `MUTATING_TOOLS` (`write`, `edit`,
   `apply_patch`, `git_commit`, `bash`, `shell`, `code_mode`,
   `external_directory`), or the shell command is read-only by heuristic
   (`is_read_only_shell`). Then:
   - agents `ask`, `plan`, `explore`, `scout` → `Refuse (intent)`;
   - a high-confidence `question` turn (or any `question` under `always`), a
     `plan` turn, or an `ambiguous` turn under `always` → `Confirm`: prompt
     with the formatted arguments plus `Intent check: …`, unless
     `risk_confirmed`;
   - otherwise allow.
5. **Path-scoped rules** — for `read`, `write`, `edit`, `glob`, `list`,
   `repomap`, `apply_patch`, `grep` with a `path` argument
   (`file_tool_path`), `PermissionSet::action_for_path` matches rules such as
   `edit(src/**)` or `write(**/*.md)` (longest glob wins; `write(...)` rules
   also cover `edit` / `apply_patch`; an `Allow` on an absolute or `..` path
   through a broad glob is coerced to `Ask`). `Deny` → error; `Allow` sets
   `risk_confirmed`; `Ask` prompts unless confirmed.
6. **Tool-name permission** — `PermissionSet::action_for(name)`:
   1. exact key in `rules`;
   2. glob keys `prefix*` or `*` — the rules map is a `HashMap`, so when
      several globs match the last one iterated wins;
   3. `denied_tools` list → `Deny`;
   4. `allowed_tools` set and name absent → `Deny`;
   5. category flags: `write` / `edit` / `apply_patch` / todo tools need
      `allow_file_writes`; `shell` / `bash` need `allow_shell`; `webfetch` /
      `websearch` / `mcp_websearch` need `allow_network`;
   6. `browser` is always `Ask`; everything else `Allow`.
   `Deny` → `Permission denied for tool '…'. Adjust agent permissions or
   config.permission.`; `Ask` → prompt unless `risk_confirmed`, with
   `permission_detail_for` as the body (see "The ask prompt"); `Allow` →
   continue.
7. **`pre_tool` hooks** — `run_pre_hooks` (`crates/plugin/src/hooks.rs`)
   runs every `[[hooks]]` entry with `event = "pre_tool"` whose `match` fits
   the tool name (`*`, `prefix*`, `*suffix`, exact). A hook with
   `block_on_failure = true` that exits non-zero, fails to spawn, or hits
   `timeout_secs` returns `pre_tool hook blocked `tool`: …` as an error
   result. Other failures are logged only.
8. **Dispatch** — `dispatch_tool`:
   - `task`, `swarm`, `bg`, `schedule`, `tool_search`, `worktree` run
     in-process (`crates/agent/src/agent/dispatch.rs`);
   - `bash` / `shell` with `background: true` → `BackgroundRegistry::start_shell`;
   - `bash` / `shell` with `[tools.bash] auto_background` (default on) →
     `execute_shell_with_auto_background`: a second catastrophic check
     (refuse), start the job, wait up to `auto_background_after_secs`, then
     either return the output or detach and return the job id;
   - everything else → `ToolExecutor::execute`
     (`crates/tools/src/executor.rs`), which re-checks
     `tool.is_allowed(permissions)` (`action_for != Deny` only) and then
     calls `Tool::execute`.
9. **Inside the tools** (`ToolContext`, `crates/core/src/tool.rs`):
   - Shell (`crates/tools/src/shell.rs`) builds a `SandboxRequest` from
     `ctx.sandbox` and runs it through `whycodes_sandbox::run_timeout`
     (timeout argument, default 120 s). `SandboxMode::Workspace` on Linux
     with `bwrap` present: root read-only, project directory read-write,
     `/tmp` a private tmpfs, selected `$HOME` subpaths and the SSH agent
     socket directory bound read-write, `--unshare-net` when `network` is
     false, `--die-with-parent`, `bash -c <command>`
     (`crates/sandbox/src/bwrap.rs`). Without `bwrap` (or on other OSes)
     `SandboxFallback::Allow` runs on the host with a one-time warning in
     the output; `Deny` fails the call with `Sandbox error: …`
     (`crates/sandbox/src/policy.rs`). `Agent::tool_context` also forces
     `sandbox.network = false` when the agent's `permission.allow_network`
     is false.
   - File writers (`write`, `edit`, `apply_patch`) call
     `ctx.check_file_write(path)`: with a `FileClaimRegistry` present
     (parallel TUI sessions via `with_session_claims`, or same-checkout swarm
     workers) the path is claimed for `agent_id`; a claim held by another
     agent returns `File conflict: … is claimed by swarm agent …` and emits
     `TurnEvent::FileConflict`. `read` calls `ctx.check_file_read`, which
     emits `TurnEvent::FileStale` when another agent wrote the file since
     this reader last saw it (`crates/core/src/file_claims.rs`).
   - HTTP tools (`webfetch`, `websearch`, GitHub issue / PR) call
     `ctx.network.check_url` (`NetworkPolicy`, `crates/core/src/network.rs`):
     denylist wins, a non-empty allowlist requires a host match, empty means
     unrestricted.
   - `external_directory` (`crates/tools/src/file/external_directory.rs`)
     accepts only paths equal to or under an entry of
     `.whycodes/external_dirs_allowed` in the working directory; an
     unreadable allowlist denies.
10. **Auto-mode retry** — in `ApprovalMode::Auto`, an error result that
    `tool_error_is_retryable` accepts (not a denial, refusal, doom-loop,
    catastrophic, missing-parameter or unknown-tool message, and not one of
    the interactive / stateful tools) is re-dispatched up to
    `AUTO_TOOL_RETRY_LIMIT` (2) more times with the same arguments.
11. **`post_tool` hooks** — `run_post_hooks` never blocks; stdout of
    successful `feedback = true` hooks is appended to the result under
    `[post_tool hook]`. Both hook events receive `WHYCODES_HOOK_EVENT`,
    `WHYCODES_TOOL_NAME`, `WHYCODES_TOOL_ID`, `WHYCODES_TOOL_INPUT`,
    `WHYCODES_SESSION_ID`, `WHYCODES_WORKING_DIR`; post adds
    `WHYCODES_TOOL_IS_ERROR` and `WHYCODES_TOOL_OUTPUT` (truncated to 8192
    characters).

The result then gets its `tool_call_id` (`answering`) and a
`TurnEvent::ToolEnd`.

```
call ──▶ question? ──▶ shell risk ──▶ shell rules ──▶ intent ──▶ path rules
         │               Refuse/Confirm   Deny/Ask       Refuse/Confirm  Deny/Ask
         ▼
      tool-name rule ──▶ pre_tool hooks ──▶ dispatch ──▶ [sandbox | claims | network]
         Deny/Ask           Block                          ──▶ auto retry ──▶ post_tool
```

### The ask prompt

`ask_permission(tc, working_dir, detail)` first consults the approval-mode
overlay (`approval_skips_ask`):

| `approval_mode` | Behaviour |
|---|---|
| `auto` (default, `ApprovalMode::default()`) | every ask is allowed without prompting |
| `important` | asks are skipped unless `approval_ask_is_high_risk`: `browser`; `bash` / `shell` / scheduled command whose risk decision is not `Allow`; a file tool whose `path` is outside the workspace (`path_outside_workspace`) |
| `manual` | every ask prompts |

`fail_closed_ask` (set for `--format json` / `stream-json`) disables the
overlay entirely, so `auto` cannot silently run a tool there. Only then is
`permission_prompter.ask(tool_name, detail)` called. The overlay never
applies to `Deny`, the catastrophic refusal, pre-tool hook blocks, or the
in-tool checks.

The `detail` body (`crates/agent/src/tool_policy.rs`):

- `edit` → `whycodes_tools::file::edit::preview_edit`: the unified diff the
  edit would produce, computed from the current file content without writing;
  `None` (file unreadable, edit would not apply) falls back to the formatted
  arguments.
- `write` → `preview_write`: the new content with line counts and, when the
  file exists, a diff against the old content.
- `bash` / `shell` risk confirm → `Command:\n…\n\nRisk: …`.
- Otherwise `format_permission_detail`: a single `command` argument is shown
  raw; other objects as `key: value` lines, multi-line strings indented.
- Everything is truncated to `PERMISSION_DETAIL_MAX` (4000 characters).

Prompters (`PermissionPrompter`, `crates/agent/src/permission.rs`):

| Prompter | Used by | Behaviour |
|---|---|---|
| `ChannelPermissionPrompter` | TUI (`crates/tui/src/run/mod.rs`) | Sends a `PermissionRequest { tool_name, detail, reply }`; the TUI queues requests in a `VecDeque` and renders `render_permission_dialog` (`crates/tui/src/ui/dialogs/confirm.rs`): `Tool`, `Command` / `Details`, `Risk`, keys `y`/`a` allow, `n`/`d`/Esc deny. A dropped channel denies. |
| `StdinPrompter` | plain REPL and text-format headless runs, via `default_prompter` | Prints `Permission required for tool …` and `Allow? [y/N]` on stderr, reads one line from stdin; `y`, `yes`, `a`, `allow` approve; EOF or read error denies. |
| `AutoApprovePrompter` / `AutoDenyPrompter` | `default_prompter` on `WHYCODES_AUTO_APPROVE` / `WHYCODES_AUTO_DENY` (`1`, `true`, `yes`) or `CI` set; structured headless runs | Constant answer. |
| `ServePrompter` | daemon (`crates/server/src/perm.rs`) | Emits `TurnEvent::PermissionAsk { request_id, tool_name, detail }` (surfaced to SDK clients as `PermissionRequest`), waits up to 300 s for the reply; timeout, missing run scope, or a closed channel deny. A session-level "always allow" and an `auto_approve` scope short-circuit to allow. |

`Agent::new` installs `default_prompter()`; hosts replace it with
`with_permission_prompter`. The denied result reads `User denied permission
for tool '…'`, or `denied:headless for tool '…'` when `fail_closed_ask` is
set; both are `is_error: true` and are excluded from auto-mode retry.

### Headless runs

`cmd_run` / `cmd_generate` with `--format json` or `stream-json`
(`crates/cli/src/cmd/run.rs`, `apply_structured_permission_policy`):
`structured_ask_is_fail_closed` is true unless `--approve-tools` or
`session.headless_ask = "allow"` (`WHYCODES_HEADLESS_ASK` is the env
overlay, `crates/config/src/load.rs`). Fail-closed installs
`AutoDenyPrompter` and `set_fail_closed_ask(true)`; otherwise
`AutoApprovePrompter`. `question` always gets `AutoAnswerPrompter`. The
`text` format keeps `default_prompter`.

### Subagents

`SubagentRunner::execute_tool_call` (`crates/agent/src/subagent.rs`) is a
different path: `question` handling, `pre_tool` / `post_tool` hooks, and
`ToolExecutor::execute` (so `Deny` and the in-tool checks of step 9 apply).
Steps 2–6 and 10 do not run for `task` / `swarm` workers. Their
`PermissionSet` comes from the parent (`general`: parent permission plus
denied todo / `task` / `swarm`; `explore` / `scout`: read-only allowlist;
custom agents: `CustomAgent::permission`).

## Configuration and overrides

| Knob | Where | Effect |
|---|---|---|
| `[permission] <tool or glob> = allow\|ask\|deny` | config (`Config::permission`), merged by `effective_permission`; agent-level `permission.rules` win | Tool-name rules (step 6); `bash(git *)` and `edit(src/**)` keys feed steps 3 and 5 |
| agent `allowed_tools`, `denied_tools`, `allow_file_writes`, `allow_shell`, `allow_network` | `PermissionSet` of the active agent (built-in agents in `crates/config/src/types.rs`, custom agents) | Lists and category flags in step 6; `allow_network = false` also cuts sandbox network |
| `[security] bash_risk_threshold` | config | `caution` / `destructive` (default) / `off`; catastrophic unaffected |
| `[security] sandbox` | config; env `WHYCODES_SANDBOX` | `off` / `workspace` (default) |
| `[security] sandbox_network` | config; env `WHYCODES_SANDBOX_NETWORK` | `--unshare-net` when false |
| `[security] sandbox_fallback` | config; env `WHYCODES_SANDBOX_FALLBACK` | `allow` (default) / `deny` when `bwrap` is missing or not Linux |
| `[security] network_allowlist`, `network_denylist` | config; env `WHYCODES_NETWORK_ALLOWLIST`, `WHYCODES_NETWORK_DENYLIST` | HTTP tool host policy (step 9) |
| `[general] approval_mode` | config; env `WHYCODES_APPROVAL_MODE`; TUI `/mode [auto\|important\|manual]` | Ask overlay; default `auto` |
| `[session] headless_ask` | config; env `WHYCODES_HEADLESS_ASK`; CLI `--approve-tools` (forces allow) | `json` / `stream-json` ask policy; `deny` is the default |
| `[session] intent_guidance` | config | `auto` / `off` / `always` for step 4 |
| `WHYCODES_AUTO_APPROVE`, `WHYCODES_AUTO_DENY`, `CI` | env, read by `default_prompter` | Only where the default prompter is in use (plain REPL, text-format headless) |
| `[[hooks]] event = "pre_tool" \| "post_tool"`, `match`, `command`, `block_on_failure`, `feedback`, `timeout_secs` | config | Steps 7 and 11; `timeout_secs` default 30 |
| `[tools.bash] auto_background`, `auto_background_after_secs` | config | Step 8 auto-detach |
| `[automation] max_background_jobs` | config | Background job cap (`BackgroundRegistry`) |
| `[swarm] worktrees` | config | Worktree isolation instead of file claims for swarm workers |
| `.whycodes/external_dirs_allowed` | file in the project directory | Allowlist for `external_directory` |
| `timeout` argument of `bash` / `shell` | tool call | Shell timeout in seconds (default 120) |

## Implementation files

- `crates/agent/src/agent/gate.rs` — `execute_tool_calls`, `execute_with_permission`, `ask_permission`, `approval_skips_ask`, `denied_permission_result`, `dispatch_tool`, auto-mode retry.
- `crates/agent/src/agent/dispatch.rs` — background / auto-background shell (second catastrophic check), `task`, `swarm` (pre-claims, worker permission profiles), `schedule`, `worktree`, `tool_search`.
- `crates/agent/src/tool_policy.rs` — `SERIAL_TOOLS`, `permission_detail_for`, `format_permission_detail`, `format_shell_risk_detail`, `file_tool_path`, `path_outside_workspace`, `PERMISSION_DETAIL_MAX`.
- `crates/agent/src/permission.rs` — `PermissionPrompter`, channel / stdin / auto prompters, `default_prompter`.
- `crates/agent/src/question.rs` — `should_prompt_questions`, question prompters.
- `crates/agent/src/intent.rs` — `authorize_tool`, `MUTATING_TOOLS`, `is_read_only_shell`.
- `crates/agent/src/subagent.rs` — `SubagentRunner::execute_tool_call` (hooks + executor only).
- `crates/core/src/types.rs` — `PermissionSet` (`action_for`, `action_for_path`, `action_for_shell`), `PermissionAction`, `ApprovalMode`.
- `crates/core/src/tool.rs` — `ToolContext`, `check_file_write`, `check_file_read`, `Tool::is_allowed`.
- `crates/core/src/file_claims.rs` — `FileClaimRegistry` (claims, stale reads).
- `crates/core/src/network.rs` — `NetworkPolicy::check_url`, host patterns.
- `crates/core/src/sandbox.rs` — `SandboxMode`, `SandboxFallback`, `SandboxSettings`.
- `crates/command-risk/src/lib.rs`, `assess.rs`, `parse.rs`, `paths.rs`, `tokenize.rs` — `RiskLevel`, `RiskThreshold`, `assess`, `decide`; `parse::parse` (shell AST lowered to `Segment`s, `ParseError` falls back to `tokenize::tokenize`).
- `crates/sandbox/src/policy.rs`, `bwrap.rs`, `host.rs` — `prepare`, `run_timeout`, bubblewrap argument list, host shell.
- `crates/tools/src/executor.rs` — `ToolExecutor::execute` (`is_allowed` re-check, formatter / LSP annotations after writes).
- `crates/tools/src/shell.rs`, `file/write.rs`, `file/edit.rs`, `file/apply_patch.rs`, `file/read.rs`, `file/external_directory.rs`, `web/fetch.rs`, `web/search.rs`, `github/*.rs` — in-tool checks.
- `crates/plugin/src/hooks.rs` — `run_hooks`, `run_pre_hooks`, `run_post_hooks`, `tool_matches`, env variables.
- `crates/config/src/types.rs` — `SecurityConfig`, `HookConfig`, `HookEvent`, `HeadlessAskMode`, `BashConfig`, `GeneralConfig::approval_mode`; `crates/config/src/load.rs` — env overlays; `merge.rs` — `effective_permission`.
- `crates/cli/src/cmd/run.rs` — `apply_structured_permission_policy`, `structured_ask_is_fail_closed`.
- `crates/tui/src/run/mod.rs`, `crates/tui/src/ui/dialogs/confirm.rs` — permission queue and dialog; `/mode`.
- `crates/server/src/perm.rs` — `ServePrompter` for daemon / SDK clients.

## Known limitations / not enforced

- The default `approval_mode` is `auto`, so with a stock config every `Ask`
  — tool-name rules, shell-risk `Confirm`, intent `Confirm`, path-rule `Ask`
  — is allowed without a prompt in the TUI and plain REPL. Only `Deny`, the
  catastrophic refusal, hook blocks and in-tool checks remain. The guide's
  Permissions section now leads with that default.
- `task` / `swarm` workers run a headless copy of the blast-radius gates
  (`SubagentRunner::headless_gate`): catastrophic commands are refused,
  shell and path `deny` rules apply, and a risk `Confirm` or a shell/path
  `ask` passes only in `auto` approval mode (other modes refuse, naming the
  mode). Tool-name `ask` rules and the intent gate are still not applied to
  workers; `general` workers inherit the parent's `bash` permission.
- `WHYCODES_AUTO_APPROVE` / `WHYCODES_AUTO_DENY` are read only by
  `default_prompter` (the plain REPL). The TUI, the daemon and structured
  headless runs install their own prompter, so the variables have no effect
  there; the guide states this scope.
- The plain prompter is interactive only when `CI` is unset **and** stdin is
  a terminal (`std::io::IsTerminal`); a piped or redirected stdin auto-denies
  instead of reading a `y` from the pipe.
- A speculative `read` that matches the final call bypasses steps 1–7 and 10
  (only hooks are honoured by disabling speculation). Path rules such as
  `read(secrets/**) = deny` are therefore not enforced on that path.
- The risk classifier is static: unrecognised commands are `Safe`, scripts
  and obfuscated commands are invisible, and only the catastrophic tier is
  path-based (`crates/command-risk/src/lib.rs` documents this). The parser
  reads bash syntax; `cmd.exe`/PowerShell lines that happen to parse are
  judged by bash quoting, as the tokenizer did. A prefix assignment is read
  as visible to its own command (`X=~ rm -rf $X` is refused) although bash
  expands `$X` first, because that is what the line was written to mean.
  Variables do not flow into `$(…)` bodies or across `cd`. A `schedule`
  command is assessed when scheduled and runs later without re-assessment.
- `action_for` glob rules live in a `HashMap`; with several matching globs
  the winner depends on iteration order. Exact keys are deterministic.
- `ToolExecutor::execute` only re-checks `Deny`; a tool added to a
  parallel-safe batch runs with the same gate results as a single call, but
  several `Ask` dialogs can be queued at once (the TUI queue is a `VecDeque`).
- Sandboxing is Linux + `bwrap` only; with the default `sandbox_fallback =
  "allow"` every other host runs commands on the host shell with a one-time
  warning. Selected `$HOME` subpaths stay writable inside the sandbox. It is
  not a multi-tenant boundary.
- The HTTP domain policy applies to `webfetch`, `websearch` (every engine in
  its `[websearch]` chain, including the `custom` endpoint), GitHub tools
  (verified) — not to the shell, `browser`, or MCP tools (MCP not verified).
- File claims are advisory and only active with parallel TUI sessions or
  same-checkout swarm; shell commands and worktree-isolated workers do not
  claim anything. Worktree merges rely on `merge_into_main` conflict detection.
- Auto-mode retry re-executes side effects: a failing `write` / `bash` is
  re-run up to two more times with identical arguments.
- `path_outside_workspace` treats a non-existent absolute path as outside
  (canonicalize fails) — this only affects the `important` high-risk check,
  it never blocks on its own.
- `ServePrompter` denies after 300 s without a reply; nothing resumes a turn
  blocked on a disconnected SDK client earlier than that.
- `browser` is always `Ask`, but under `auto` that ask is skipped like any
  other; the tool's own notes say network allowlists do not apply inside it.

## Related

- [turn-loop.md](turn-loop.md) — where `execute_tool_calls` sits in the turn.
- Guide: [Permissions](../guide.md#permissions), [Shell command risk](../guide.md#shell-command-risk), [Shell OS sandbox](../guide.md#shell-os-sandbox), [Network allowlist (HTTP tools)](../guide.md#network-allowlist-http-tools), [Hooks](../guide.md#hooks), [Output formats (headless / CI)](../guide.md#output-formats-headless--ci), [Interactive session](../guide.md#interactive-session).
- [mcp-lifecycle.md](mcp-lifecycle.md) — MCP tools bind as `{server}_{tool}` and pass the same tool-name rules.
- [architecture.md](../architecture.md) — `command-risk` and `sandbox` are foundation crates; `agent` is orchestration.
- [knowhow.md](../knowhow.md) — "Auto mode must not ask while todos/tasks are open", "Default TUI `auto` never opened the question panel".
