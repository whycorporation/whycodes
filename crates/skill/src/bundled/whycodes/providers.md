# Providers

`[providers.<name>]` tables hold `name`, `api_base` / `base_url`, `api_key`
(prefer an env variable), `models`, `headers`, `extra_body`.
`default_model = { provider_id, model_id }` picks the session route;
`-P` / `-m` override it for one run; `/models provider/id` switches live.

Commands: `whycodes provider list|add|remove|default|discover`,
`whycodes model list`, `whycodes model default <provider> <model>`.
`provider discover` probes local servers on 127.0.0.1 (ports 1234, 8000,
8080); `--save` writes them.

Credential order for a provider: environment variable
(`<PROVIDER>_API_KEY`), then config `api_key`, then a stored subscription
login (OAuth, `whycodes auth login <provider>`, needs an auth plugin). Named
credentials: `[providers.<id>.credentials]`. Local and loopback servers need
no key. `whycodes auth status --json` says which source wins, never the
secret.

Never print, echo, or ask the user to paste a key into the chat. To set
one, tell the user to export the env variable, use `/connect`, or run
`whycodes config set providers.<id>.api_key <value>` themselves (write-only,
never echoed).

Model roles under `[session]`: `model_smol` (workers), `model_plan`
(`plan` agent), `model_fast`, `model_race`, `fallback_models`,
`reasoning_effort`, `prewalk`.

Details: docs/providers.md (per-provider behaviour, "Shared rules");
docs/auth.md; docs/config.md "[providers.<name>]", "[session]".
