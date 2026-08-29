# Security Policy

## Reporting a vulnerability

Please do **not** open a public issue for security problems.

Use GitHub's private reporting channel:
[github.com/whycorporation/whycodes/security/advisories/new](https://github.com/whycorporation/whycodes/security/advisories/new)

Include enough detail to reproduce: whycodes version (`whycodes --version`), OS,
configuration (redact credentials), and the input or sequence that triggers
the problem. You can expect an acknowledgement within a few days; fixes ship
in the next release after the report is confirmed.

## Supported versions

Only the latest tagged release receives security fixes. Interactive sessions
self-update to it unless disabled; `whycodes upgrade` does the same on demand.

## Scope

whycodes is an agent: it runs shell commands and edits files **by design**, on
behalf of a model whose output is not trusted input. The following are treated
as security vulnerabilities:

- Bypassing the shell command risk classification
  (`safe`/`caution`/`destructive`/`catastrophic`) so that a command executes
  in a lower tier than intended, or a `catastrophic` command executes at all
- Escaping the OS sandbox (`crates/sandbox`) from a sandboxed shell invocation
- Circumventing `allow`/`ask`/`deny` tool permissions or the HTTP domain
  allowlist from model-controlled input
- Reading, exfiltrating or weakening the storage of credentials under the
  whycodes data directory (API keys, OAuth tokens — stored `0600`, symlink
  refused)
- The self-update path installing a binary whose checksum does not match the
  release's `SHA256SUMS`

The built-in `browser` tool launches a real Chromium. That process is
**outside** the shell OS sandbox and is **not** filtered by the HTTP domain
allowlist. Treat it as an `ask`-gated capability, not a confinement boundary.

Out of scope: a model producing unwanted but correctly-gated actions (that is
a model behaviour, not a whycodes vulnerability — tune `bash_risk_threshold`
and permissions), and issues in third-party LLM providers themselves.

## What this repository does not contain

Credentials live in the user data directory (`0600`, symlink refused), not
in git. The index is checked by `scripts/check_tracked_secrets.py` (scratch
dirs, private keys, live-looking PATs). The repository ships **no** OAuth
client ids at all: subscription login only works after the user installs a
local `kind: "auth"` plugin (see `docs/auth.md`), so there is nothing
client-id-shaped in `crates/auth` to protect or leak.

Dependency advisories are checked in CI with `cargo audit --deny warnings`
against the RustSec database. Exceptions live in `.cargo/audit.toml` as a
ratchet: each entry is a dated, justified unmaintained-crate notice on a
transitive dependency — actual vulnerabilities are never ignored there.

Commit author emails are part of history and are **not** rewritten. Do not
force-push a history filter unless a live credential actually landed in a
blob.

This repository is public. Fork pull requests do not execute on the project's
self-hosted runner. Production deploy and release jobs are pinned to
`whycorporation/whycodes` so a fork cannot reuse those workflows with this
repo's secrets.
