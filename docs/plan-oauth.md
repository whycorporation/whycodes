# Plan — OAuth and credential discovery

**Status:** blocked (2026-07-31) · **Was:** phase 3 · **Depends on:** first install/release path ([plan-distribution.md](plan-distribution.md)) · **Blocks:** nothing

## Why this is blocked rather than in progress

Both halves of this phase turn on a question that cannot be answered by writing
code, and getting it wrong has consequences beyond a bug.

**The OAuth flows need a registered client.** A device flow is not something a
third-party client can simply perform against Anthropic or OpenAI; it needs a
client identifier issued to whycode. Registering one is an act by the project
owner, under whatever terms the provider attaches.

**Credential discovery may not be permitted at all.** Reading Claude Code's or
Codex's stored OAuth token and using it through a different client is precisely
the thing a provider's terms are likely to address. The risks section below
already flagged it — *"using a subscription credential through a third-party
client may violate a provider's terms. Check each provider's terms before
shipping its flow… If a flow is not permitted, do not ship it."* That check has
not been done, and it is a reading of legal terms, not an engineering task.

Implementing the consent model and the token store first would be building the
machinery for something that might not be allowed to ship. So the phase waits on
two owner decisions:

1. Register an OAuth client with Anthropic and OpenAI, or decide not to.
2. Read each provider's terms on third-party use of an existing credential, and
   record per provider whether discovery is permitted.

Once those are answered, the tasks below are ordinary work. Until then, API keys
remain the only supported path, which is at least honest about what it is.

## Problem

whycode authenticates with API keys only — an environment variable or an
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

`whycode` on a fresh machine reaches a working session without the user
visiting a provider console, and never reads another tool's credentials
without being told to.

## Scope

In:

- `whycode login --provider <name>`: OAuth device flow for Anthropic and
  OpenAI, storing tokens under the whycode data directory.
- Token refresh, transparently, before expiry.
- Credential discovery: detect credential files belonging to other CLIs,
  list what was found, and import only after explicit per-path approval.
- `whycode logout --provider <name>`.
- `whycode debug` reports which providers are authenticated and by what
  method, without printing secrets.

Out:

- OS keychain storage. Start with file permissions (`0600` on Unix, ACL on
  Windows) and revisit. macOS Claude Code credentials live in the login
  Keychain, so *reading* that is in scope but *writing* our own is not.
- Copilot, Gemini, Azure, Bedrock. Add after the first two flows are proven.
- A credential-sharing daemon.

## Tasks

- [ ] `crates/auth`: token storage, expiry tracking, refresh
- [ ] File permissions on the token store: `0600` on Unix, restrictive ACL on
      Windows; refuse to use a world-readable store
- [ ] Anthropic OAuth device flow
- [ ] OpenAI OAuth device flow
- [ ] Automatic refresh, with a single retry on a 401 that looks like expiry
- [ ] Discovery: locate known credential paths per platform, report findings
- [ ] Consent prompt per source path, with the decision persisted
- [ ] Reject symlinked credential sources; never write to a discovered file
- [ ] `login`, `logout`, and the `debug` reporting
- [ ] `/connect` in the TUI offers login instead of only printing help
- [ ] `docs/auth.md` documenting every path read and every file written

## Acceptance criteria

- [ ] `whycode login --provider anthropic` completes a device flow and a
      subsequent `whycode generate "hi"` works with no API key set
- [ ] An expired access token refreshes without user interaction
- [ ] Discovery finds a Claude Code credential file when present and does
      **not** read it until approved
- [ ] Approving a source persists, so the prompt does not reappear
- [ ] A symlinked credential source is refused with a clear message
- [ ] No discovered file is modified — verified by comparing mtime and content
      hash before and after a session
- [ ] `whycode debug` shows auth state and never prints a token, not even
      truncated
- [ ] Secrets do not appear in logs at any tracing level

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
