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
| `openai` | PKCE → loopback callback on the registered port `localhost:1455` | ⚠️ login + refresh work; ChatGPT-subscription tokens only authorize the Codex backend (`chatgpt.com/backend-api`), not `api.openai.com` — call routing is a follow-up |
| `github-copilot` | GitHub device-code grant → GitHub token is exchanged for the short-lived Copilot API token | ✅ yes — `github-copilot` provider calls `api.githubcopilot.com/chat/completions`; the Copilot token re-exchanges automatically near expiry |
| `google` | PKCE → loopback callback on an ephemeral port | ⚠️ login + refresh work; Gemini-subscription calls need the Code Assist endpoint (`cloudcode-pa.googleapis.com`), not the API-key `generativelanguage` route — call routing is a follow-up |

Expired access tokens refresh transparently on next use (GitHub's token
does not expire; the derived Copilot token does and is re-exchanged).

## Provider terms caveat

Using a subscription credential through a third-party client is a matter
of each provider's terms of service, which change over time and which this
project does not interpret for you. If a provider's terms do not permit
third-party use of its subscription token, do not use that flow. API keys
remain the fully-supported, unambiguous path.
