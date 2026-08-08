# Authentication — API keys and OAuth subscription login

whycode accepts two credential kinds per provider:

1. **API keys** — env var (`ANTHROPIC_API_KEY`, …) or `api_key` in `config.toml`.
2. **OAuth subscription login** — `whycode auth login <provider>` stores a
   token from an existing subscription (Claude Pro/Max, ChatGPT Plus/Pro,
   GitHub Copilot, Google/Gemini).

Resolution order is always: **env var → config `api_key` → OAuth store**.
An explicit key therefore never loses to a stored subscription login.

## Commands

```bash
whycode auth login anthropic          # browser sign-in (Claude Pro/Max)
whycode auth login openai             # browser sign-in (ChatGPT Plus/Pro)
whycode auth login github-copilot     # device code on github.com
whycode auth login google             # browser sign-in (Gemini)
whycode auth login <p> --no-browser   # print the URL instead of opening it
whycode auth status                   # who is logged in (never prints tokens)
whycode auth logout <provider>        # remove stored credential
```

`whycode debug` also lists stored logins (method + expiry only).

In the TUI, `/connect` reloads the credential for the active provider; when
none exists and the provider supports OAuth, it starts the login flow
in-place — the browser opens from the TUI, loopback/device flows report
progress in the transcript, and Anthropic's paste flow collects
`code#state` through the prompt box (Esc cancels).

## What is stored, and where

| Path | Contents | Permissions |
|------|----------|-------------|
| `<data_dir>/auth.json` | OAuth access/refresh tokens per provider | `0600` (owner-only; a looser file is refused) |

`<data_dir>` is the platform data dir (`~/.local/share/whycode` on Linux,
`~/Library/Application Support/com.whycorporation.whycode` on macOS).
Writes are atomic (temp file + rename). Tokens never appear in logs or
`Debug` output at any level.

## Flow per provider

The flows use the public OAuth client ids that ship in the first-party /
community CLIs — whycode has no registered client of its own.

| Provider | Flow | Works for API calls |
|----------|------|---------------------|
| `anthropic` | PKCE; the public client's redirect shows `code#state` on a console page → paste it into the terminal | ✅ yes — token sent as `Authorization: Bearer` + `anthropic-beta: oauth-2025-04-20` |
| `openai` | PKCE → loopback callback on the registered port `localhost:1455` | ✅ yes — JWT-shaped subscription tokens are routed to the Codex backend (`chatgpt.com/backend-api/codex/responses`, Responses API) with the stored `chatgpt-account-id`; API keys keep the `api.openai.com` chat-completions path (`crates/llm/src/codex.rs`) |
| `github-copilot` | GitHub device-code grant → GitHub token is exchanged for the short-lived Copilot API token | ✅ yes — `github-copilot` provider calls `api.githubcopilot.com/chat/completions`; the Copilot token re-exchanges automatically near expiry |
| `google` | PKCE → loopback callback on an ephemeral port | ✅ yes — `ya29.…` OAuth tokens are routed to the Code Assist endpoint (`cloudcode-pa.googleapis.com/v1internal`) with `loadCodeAssist`/`onboardUser` project discovery (`GOOGLE_CLOUD_PROJECT` overrides); `AIza…` API keys keep the `generativelanguage` route (`crates/llm/src/codeassist.rs`) |

Expired access tokens refresh transparently on next use (GitHub's token
does not expire; the derived Copilot token does and is re-exchanged). If a
provider still answers 401 on a token the store considered fresh, the
credential is force-renewed and the request retried exactly once; a second
401 surfaces as a normal error.

## Adding a new OAuth provider (standard)

The design goal: adding a provider is **only** adding one `ProviderSpec`
literal plus one registry entry — no branches in flow code. The conformance
suite (`cargo test -p whycode-auth`) rejects a malformed spec before it can
misbehave at runtime.

Checklist:

1. **`crates/auth/src/lib.rs`** — add the name to `OAUTH_PROVIDERS`.
2. **`crates/auth/src/providers.rs` → `spec_for()`** — add one match arm:
   - Pick the `flow`: `LoopbackPkce` (browser → localhost callback),
     `PasteCodePkce` (registered redirect is a fixed web page → user pastes
     `code#state`), or `DeviceCode` (RFC 8628 grant).
   - Set `token_encoding` (`Form` = RFC 6749 default, `Json` for
     Anthropic-style endpoints) — grant bodies are built from this, never
     from the provider name.
   - `redirect_uri`: `Some(..)` for paste flows, `None` for loopback
     (constructed from the bound port).
   - `derived`: `Some(DerivedCredential { .. })` when the API credential
     comes from a second exchange (Copilot model), else `None`.
3. **`crates/auth/src/error.rs`** — add the name to the
   `UnsupportedProvider` message (tripwire test enforces this).
4. **`docs/auth.md`** — add a row to the flow table above.

The suite then verifies, for every advertised provider: `validate()`
invariants (https URLs, flow/redirect consistency, unique authorize extras,
derived-exchange description), the authorize URL carries all required PKCE
params and never the verifier, and the grant bodies match the declared
`token_encoding` (including `client_secret` presence). CLI tests
(`crates/cli/tests/cli_args.rs`) pin the `auth` surface offline.

If the provider's *API calls* need a nonstandard endpoint or headers, that
is a separate, explicit step: a provider module in `crates/llm` (see
`copilot.rs`) registered in `provider.rs`. Login/refresh/storage never
depend on it.

## Provider terms caveat

Using a subscription credential through a third-party client is a matter
of each provider's terms of service, which change over time and which this
project does not interpret for you. If a provider's terms do not permit
third-party use of its subscription token, do not use that flow. API keys
remain the fully-supported, unambiguous path.
