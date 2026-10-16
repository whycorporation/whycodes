# Errata

> Status: Living · Updated: 2026-10-16

Statements WhyCodes documentation or code once made that turned out to be
wrong or were reversed. Each entry keeps the original claim, the evidence
that changed it, and what holds now. `knowhow.md` records bugs that are easy
to reintroduce; this page records **claims** that are easy to repeat.

Add an entry when a release note, guide sentence or default is retracted,
not merely extended. Keep the old wording verbatim so a reader who saw it
can match it.

## Format

```
### YYYY-MM-DD — <one-line title>

**Claimed (where, when):** the sentence as it appeared.
**Evidence:** what showed it was wrong (bug, measurement, user report, commit).
**Now:** the corrected statement and where it is documented.
**Historical note:** anything still true about the old behaviour (versions it applied to).
```

## Entries

### 2026-09-20 — The instance root is `~/.whycodes` on every OS

**Claimed (guide and README, 0.6.4 and earlier):** config and data live in
the platform directory: `~/.config/whycodes` or
`~/.config/com.whycorporation.whycodes` on Linux,
`~/Library/Application Support/com.whycorporation.whycodes` on macOS,
`%APPDATA%\whycorporation\whycodes` on Windows.

**Evidence:** three different roots across OSes made every support
exchange start with "where is your config?", and the Windows path was not
the one most users looked in. See `knowhow.md`, entry
"2026-09-20 — Default instance is `~/.whycodes`, not ProjectDirs".

**Now:** `~/.whycodes` (`%USERPROFILE%\.whycodes` on Windows), or
`$WHYCODES_HOME` when set, on every OS. The first 0.6.5 launch copies
`config.toml`, `auth.json` and `whycodes.db` across when the new directory
is empty and leaves the old files in place. Documented in
[guide.md › Configuration](guide.md#configuration) and
[config.md](config.md).

**Historical note:** 0.6.4 and earlier read the platform directories. A
set `WHYCODES_HOME` skips the copy.

### 2026-08-27 — Subscription login needs an auth plugin

**Claimed (guide, before 2026-08-27):** `whycodes auth login <provider>`
signs in with an existing subscription.

**Evidence:** shipping third-party OAuth client registrations inside the
binary tied the release to credentials WhyCodes does not own. Commit
`87f46b0d` moved OAuth specs out of core.

**Now:** `whycodes auth login` works only after a local plugin with
`"kind": "auth"` is installed; the default install ships no OAuth clients.
Resolution order stays env var → config `api_key` → OAuth store. Documented
in [auth.md](auth.md) and [guide.md › Subscription login](guide.md#subscription-login).

**Historical note:** builds before that commit contained the specs and the
command worked without a plugin.

## Related

- [knowhow.md](knowhow.md) — bugs and traps, as opposed to retracted claims.
- [roadmap.md](roadmap.md) — standing decisions that are still in force.
