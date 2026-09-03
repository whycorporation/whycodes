# Authentication — API keys and OAuth subscription login

whycodes accepts two credential kinds per provider:

1. **API keys** — env var (`ANTHROPIC_API_KEY`, …) or `api_key` in `config.toml`.
2. **OAuth subscription login** — only after an **auth plugin** is installed.
   Built-in WhyCodes has **no** subscription-login clients and does not
   impersonate another product's User-Agent. `whycodes auth login <provider>`
   stores a token from an existing subscription when a matching plugin is
   loaded (Claude Pro/Max, ChatGPT Plus/Pro, GitHub Copilot, Google/Gemini,
   Google Antigravity, xAI SuperGrok / X Premium).

Resolution order is always: **env var → config `api_key` → OAuth store**.
An explicit key therefore never loses to a stored subscription login.

## Auth plugins

OAuth specs come from `plugin.json` with `"kind": "auth"`. Drop a plugin
directory into `~/.config/com.whycorporation.whycodes/plugins/` or
`<project>/.whycodes/plugins/`. The CLI loads those dirs at startup;
`kind: "auth"` plugins never become shell tools.

WhyCodes does **not** ship subscription-login plugins. Install a local
`kind: "auth"` plugin if you choose to; third-party OAuth clients may
violate that provider's terms and are not part of this repository.

Until a plugin is installed, `whycodes auth login`, `/login`, and
`auth status` report that no OAuth providers are registered.

## Commands

```bash
whycodes auth login <provider>         # after installing an auth plugin
whycodes auth login <p> --no-browser   # print the URL instead of opening it
whycodes auth status                   # who is logged in (never prints tokens)
whycodes auth logout <provider>        # remove stored credential
```

`whycodes debug` also lists stored logins (method + expiry only).

In the TUI, `/connect` reloads the credential for the active provider; when
none exists and the provider supports OAuth (a plugin is loaded), it starts
the login flow in-place — the browser opens from the TUI, loopback/device
flows report progress in the transcript, and Anthropic's paste flow collects
`code#state` through the prompt box (Esc cancels). `/login` opens a provider
picker of **currently loaded** auth plugins, annotated with which
subscriptions are already connected, and starts the same flow for the chosen
one. Subscription backends do not expose a listable `/models` endpoint, so
after a login the `/models` picker offers each connected provider's
suggested models (`whycodes_auth::suggested_models`).

## What is stored, and where

| Path | Contents | Permissions |
|------|----------|-------------|
| `<data_dir>/auth.json` | OAuth access/refresh tokens per provider | `0600` (owner-only; a looser file is refused) |
| `<data_dir>/auth-consent.json` | Per-path approve/deny decisions for credential import | `0600` |

`<data_dir>` is the platform data dir (`~/.local/share/whycodes` on Linux,
`~/Library/Application Support/com.whycorporation.whycodes` on macOS).
Writes are atomic (temp file + rename). Tokens never appear in logs or
`Debug` output at any level.

## Flow per provider

The OAuth engine in `whycodes-auth` is generic (PKCE paste-code, loopback,
device-code). Client ids, redirect ports, and optional inference identity
come from the installed plugin — core never ships them.

When a plugin registers one of these provider names, LLM routing for that
token is:

| Provider | Flow | Works for API calls |
|----------|------|---------------------|
| `anthropic` | PKCE; the public client's redirect shows `code#state` on a console page → paste it into the terminal | ✅ yes — token sent as `Authorization: Bearer` + `anthropic-beta: oauth-2025-04-20` |
| `openai` | PKCE → loopback callback on the registered port `localhost:1455` | ✅ yes — JWT-shaped subscription tokens are routed to the Codex backend (`chatgpt.com/backend-api/codex/responses`, Responses API) with the stored `chatgpt-account-id`; API keys keep the `api.openai.com` chat-completions path (`crates/llm/src/codex.rs`) |
| `github-copilot` | GitHub device-code grant → GitHub token is exchanged for the short-lived Copilot API token | ✅ yes — `github-copilot` provider calls `api.githubcopilot.com/chat/completions`; the Copilot token re-exchanges automatically near expiry |
| `google` | PKCE → loopback callback on an ephemeral port | ✅ yes — `ya29.…` OAuth tokens are routed to the Code Assist endpoint (`cloudcode-pa.googleapis.com/v1internal`) with `loadCodeAssist`/`onboardUser` project discovery (`GOOGLE_CLOUD_PROJECT` overrides); `AIza…` API keys keep the `generativelanguage` route (`crates/llm/src/codeassist.rs`) |
| `google-antigravity` | PKCE → loopback callback on the native hub port `127.0.0.1:51121` (`/oauth-callback`); Antigravity client id + scopes (`cclog`, `experimentsandconfigs`) | ✅ yes — tokens go to `daily-cloudcode-pa.googleapis.com` with `ideType: ANTIGRAVITY`. A loaded plugin may supply the hub User-Agent via `inference.user_agent`. Picker ids such as `gemini-3.1-pro` remap to hub wire ids (`gemini-3.1-pro-low`). Distinct from `google` (Gemini CLI / Code Assist sunset for consumer accounts). |
| `xai` | PKCE → loopback callback on an ephemeral `127.0.0.1` port (`/callback`); public Grok Build client | ✅ yes — SuperGrok / X Premium tokens go to `cli-chat-proxy.grok.com`. Extra proxy headers (`X-XAI-Token-Auth`, …) come from the plugin's `inference` object. Console keys (`xai-…`) stay on `api.x.ai` (`crates/llm/src/providers/xai.rs`) |

Expired access tokens refresh transparently on next use (GitHub's token
does not expire; the derived Copilot token does and is re-exchanged). If a
provider still answers 401 on a token the store considered fresh, the
credential is force-renewed and the request retried exactly once; a second
401 surfaces as a normal error.

## Credential import (discovery)

`whycodes auth import` scans for credential files written by other CLIs and
imports them **only after explicit per-path approval**:

| Source | File read | Provider |
|--------|-----------|----------|
| Claude Code | `~/.claude/.credentials.json` | `anthropic` |
| Codex CLI | `~/.codex/auth.json` | `openai` |
| Gemini CLI | `~/.gemini/oauth_creds.json` | `google` |
| GitHub Copilot | `~/.config/github-copilot/hosts.json` | `github-copilot` |
| Grok Build | `~/.grok/auth.json` | `xai` |

The consent model (mirroring jcode's `OAUTH.md`):

- Discovery reports existence **without reading contents**; a file is read
  only after you answer `y` for that exact path, and the decision is
  persisted in `auth-consent.json` so the prompt never reappears.
- A source file is **never modified** — no move, rewrite, or permission
  change (a test pins content-hash + mtime stability across an import).
- **Symlinked sources are refused**, even with approval.
- Imported credentials are stored in `auth.json` with method `imported`;
  refresh and the 401 re-auth path work the same as for OAuth logins.
  Codex imports also carry `account_id` so the Codex backend route gets
  its `chatgpt-account-id` header.
- macOS Keychain entries (Claude Code's store on that platform) are out of
  scope; use `whycodes auth login anthropic` there.

To reset a decision, delete `auth-consent.json` (or edit out the path).

## Settings import (MCP, permissions, hooks)

`whycodes import` copies **user-level** MCP servers, permission rules, and
hooks from Claude Code, OpenCode, Grok Build, and Codex CLI into
`config.toml`. Project files (`AGENTS.md`, `.mcp.json`, `.claude/skills/`)
are not copied — they stay live in the repo.

```bash
whycodes import              # prompt per new source, then per MCP/permission/hook
whycodes import --dry-run    # plan only
whycodes import --from claude --yes   # copy everything without prompts
```

Same consent model as credential import: a path is read only after approval,
the source is never modified, and symlinks are refused. Existing WhyCodes
keys win unless `--force`. Credentials stay on `whycodes auth import`.

On first interactive run, if `config.toml` is missing and another agent is
found, WhyCodes asks once: the full-screen TUI opens a checkbox picker
(Space toggle, Enter apply); `--plain` asks y/N then Y/n per item.
`/import` (or `whycodes import`) runs the same copy later. `WHYCODES_SKIP_IMPORT=1`
or `CI=1` skips the prompt. Piped stdin is also skipped so CI is never blocked.

## Adding a new OAuth provider

The design goal: adding a provider is **installing a `kind: "auth"`
plugin** — no branches in flow code, no new match arm in core. The
conformance suite (`cargo test -p whycodes-auth`) rejects a malformed spec
before it can misbehave at runtime.

Checklist:

1. **Write `plugin.json`** with `"kind": "auth"` and an `auth` object:
   - `provider` — store / CLI name (`anthropic`, `openai`, …).
   - `flow`: `loopback-pkce` (browser → localhost callback),
     `paste-code-pkce` (registered redirect is a fixed web page → user pastes
     `code#state`), or `device-code` (RFC 8628 grant).
   - `token_encoding` (`form` = RFC 6749 default, `json` for
     Anthropic-style endpoints) — grant bodies are built from this, never
     from the provider name.
   - `redirect_uri`: set for paste flows; omit for loopback
     (constructed from the bound port).
   - `derived`: when the API credential comes from a second exchange
     (Copilot model).
   - Optional `inference`: User-Agent / extra headers for LLM calls that
     use this plugin's token. Core traffic stays `whycodes/<version>`
     without this object.
2. **Install** the directory under the user or project plugins folder.
   Restart (or start a new `whycodes` process) so `load_from_dirs` registers
   it. `whycodes auth login <provider>` then lists it.
3. **`docs/auth.md`** — add a row to the flow table above if the LLM
   crate already routes that provider name.

The suite then verifies, for synthetic fixture specs covering each flow
kind: `validate()` invariants (https URLs, flow/redirect consistency,
unique authorize extras, derived-exchange description), the authorize URL
carries all required PKCE params and never the verifier, and the grant
bodies match the declared `token_encoding` (including `client_secret`
presence). CLI tests (`crates/cli/tests/cli_args.rs`) pin the `auth`
surface offline.

If the provider's *API calls* need a nonstandard endpoint or headers, that
is a separate, explicit step: a provider module in `crates/llm` (see
`copilot.rs`) registered in `provider.rs`. Login/refresh/storage never
depend on it. Impersonating another product's identity belongs in the
plugin's `inference` object, not in core.

## Provider terms caveat

Using a subscription credential through a third-party client is a matter
of each provider's terms of service, which change over time and which this
project does not interpret for you. If a provider's terms do not permit
third-party use of its subscription token, do not use that flow. API keys
remain the fully-supported, unambiguous path.
