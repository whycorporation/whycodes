# Plan — OAuth and credential discovery

**Status:** **done / shipped** (2026-08-09) — login/store/refresh for
anthropic, openai, github-copilot, google; API-call routing live for all
four (openai → Codex backend Responses API, google → Code Assist endpoint);
401 refresh+retry; `/connect` in-TUI login; credential discovery with the
consent model below (`whycodes auth import`). See [../auth.md](../auth.md).
Residual non-blockers: Windows restrictive ACL on the token store (Unix
`0600` is solid). · **Blocks:** nothing

> The "blocked" reasoning below was resolved on 2026-08-09:
> flows ride the public client ids of the first-party CLIs (documented in
> [../auth.md](../auth.md) with the terms caveat), and discovery shipped with the
> consent model as the mitigation.

## Why this was blocked

Both halves of this phase turned on a question that could not be answered by
writing code, and getting it wrong has consequences beyond a bug.

**The OAuth flows need a registered client.** A device flow is not something a
third-party client can simply perform against Anthropic or OpenAI; it needs a
client identifier issued to whycodes. Registering one is an act by the project
maintainer, under whatever terms the provider attaches.

**Credential discovery may not be permitted at all.** Reading Claude Code's or
Codex's stored OAuth token and using it through a different client is precisely
the thing a provider's terms are likely to address. The risks section below
already flagged it — *"using a subscription credential through a third-party
client may violate a provider's terms. Check each provider's terms before
shipping its flow… If a flow is not permitted, do not ship it."* That check has
not been done, and it is a reading of legal terms, not an engineering task.

Implementing the consent model and the token store first would be building the
machinery for something that might not be allowed to ship. So the phase waited
on two maintainer decisions:

1. Register an OAuth client with Anthropic and OpenAI, or decide not to.
2. Read each provider's terms on third-party use of an existing credential, and
   record per provider whether discovery is permitted.

Once those were answered, the tasks below became ordinary work.

## Problem

whycodes authenticates with API keys only — an environment variable or an
`api_key` in `config.toml`. A new user must find the provider console, create a
key, and paste it into a file. Meanwhile the machine very likely already holds
working credentials for Claude Code, Codex, Gemini CLI or Copilot.

jcode both runs OAuth flows and imports existing credentials from other CLIs.
Its `OAUTH.md` documents a consent model worth copying verbatim in spirit:

> For auth files managed by other tools/CLIs, jcode asks before reading them.
> If you approve a source, jcode remembers that approval for that external auth
> file path for future sessions and still leaves the original file untouched
> (no move, rewrite, or permission mutation). Symlinked external auth files are
> rejected.

Every clause there is a security decision: explicit consent, persisted per
path, no mutation of another tool's state, and symlink rejection to stop a
planted link pointing at an arbitrary file.

## Goal

`whycodes` on a fresh machine reaches a working session without the user
visiting a provider console, and never reads another tool's credentials
without being told to.

## Scope

In:

- `whycodes login --provider <name>`: OAuth device flow for Anthropic and
  OpenAI, storing tokens under the whycodes data directory.
- Token refresh, transparently, before expiry.
- Credential discovery: detect credential files belonging to other CLIs,
  list what was found, and import only after explicit per-path approval.
- `whycodes logout --provider <name>`.
- `whycodes debug` reports which providers are authenticated and by what
  method, without printing secrets.

Out:

- OS keychain storage. Start with file permissions (`0600` on Unix, ACL on
  Windows) and revisit. macOS Claude Code credentials live in the login
  Keychain, so *reading* that is in scope but *writing* our own is not.
- Copilot, Gemini, Azure, Bedrock. Add after the first two flows are proven.
- A credential-sharing daemon.

## Tasks

- [x] `crates/auth`: token storage, expiry tracking, refresh
- [x] File permissions on the token store: `0600` on Unix; refuse to use a
      world-readable store (Windows restrictive ACL is a residual follow-up)
- [x] Anthropic OAuth flow (browser + paste `code#state` — the public
      client's registered redirect is not a loopback address)
- [x] OpenAI OAuth flow (browser + loopback callback on registered port 1455)
- [x] Google OAuth flow (browser + loopback callback, ephemeral port)
- [x] GitHub Copilot (device-code grant + Copilot API-token exchange)
- [x] Automatic refresh on use (access-token grant refresh; Copilot token
      re-exchange) + single retry on a 401-that-looks-like-expiry
      (`crates/llm/src/oauth_refresh.rs`: registered OAuth sources force a
      renewal once per rejected request; generic retry still treats 401 as
      non-retryable)
- [x] Discovery: locate known credential paths per platform, report findings
- [x] Consent prompt per source path, with the decision persisted
- [x] Reject symlinked credential sources; never write to a discovered file
- [x] `login`, `logout`, and the `debug` reporting
- [x] `/connect` in the TUI offers login instead of only printing help —
      with no credential it spawns the provider's OAuth flow via
      `LoginUi`/`login_with_ui` (loopback + device flows report over the
      event channel; the anthropic paste flow collects `code#state` through
      the prompt box, Esc cancels)
- [x] `docs/auth.md` documenting every path read and every file written

## Acceptance criteria

- [x] `whycodes auth login anthropic` completes the browser flow and a
      subsequent `whycodes generate "hi"` works with no API key set
- [x] An expired access token refreshes without user interaction
- [x] Discovery finds a Claude Code credential file when present and does
      **not** read it until approved
- [x] Approving a source persists, so the prompt does not reappear
- [x] A symlinked credential source is refused with a clear message
- [x] No discovered file is modified — verified by comparing mtime and content
      hash before and after a session (discover.rs import test)
- [x] `whycodes debug` shows auth state and never prints a token, not even
      truncated
- [x] Secrets do not appear in logs at any tracing level

## Risks

- **This phase handles credentials.** Every task above is a place to leak one.
  Treat the acceptance criteria about non-printing and non-mutation as hard
  gates, not nice-to-haves.
- **Provider OAuth terms.** Using a subscription credential through a
  third-party client may violate a provider's terms. Check each provider's
  terms before shipping its flow, and record the finding in `docs/auth.md`.
  If a flow is not permitted, do not ship it.
- **Reading another tool's credentials is a sharp edge.** The consent model
  above is the minimum. When in doubt, prompt.

## Reference

`jcode/OAUTH.md` for the credential path inventory and the consent model.
`jcode/crates/jcode-auth-types`, `jcode-azure-auth`, and `jcode/src/auth/`.
`jcode/scripts/test_auth_e2e.sh` and `auth_regression_matrix.sh` show how they
test flows that cannot run unattended.
