# Providers

> Status: Implemented · Updated: 2026-10-16

This is the catalogue of LLM provider families built into `whycodes-llm`
and the quirks each one carries: how its request body is built, how its
stream is parsed, how it authenticates and how its models are described.
Every entry follows the code in `crates/llm/src/providers/<name>.rs` and
the shared modules it calls. Anything not visible in the code is marked
"not verified".

To add an entry, add a `### <provider>` section under **Catalogue** with
the four fixed sub-headings, cite the file and the functions that make the
behaviour special, and follow the **Adding a provider** checklist. Credential
flows (API keys, OAuth plugins, import) are documented in
[`auth.md`](auth.md); this page only states which credential each provider
accepts and where it goes on the wire.

## Shared rules

**`/v1/models` context window** (restated from `knowhow.md`, hard rule 5).
The live model list is fetched only from the active provider's config:
`catalog_request_from_config` (`crates/llm/src/model_catalog.rs`) needs a
`base_url` / `api_base` and uses config `api_key`, the runtime key, or
`<PROVIDER>_API_KEY`; there are no hard-coded gateway hosts. The TUI calls
`fetch_model_context_window` for the active model only (3 s timeout, 8 MB
body cap) and keeps one `api_context_window`, never the whole catalogue.
Failure is non-fatal: `resolve_context_window` (`crates/llm/src/capabilities.rs`)
falls back to the built-in table, then name heuristics, then
`session.max_context_tokens`. `WHYCODES_NO_MODEL_CATALOG=1` skips the fetch
(`crates/tui/src/run/mod.rs`).

**`max_tokens` vs `context_window`** (hard rule 6). `ModelConfig.max_tokens`
(`crates/core/src/types.rs`) is the completion cap sent to the API;
`ModelConfig.context_window` (or the catalogue's `context_length`) is the
full prompt+completion budget used by the meter and compaction. Rate-limit
headers such as `x-ratelimit-limit-tokens` are TPM quotas and are never used
as a context window. Explicit `context_window` in config wins over the
catalogue, which wins over the built-in table (`resolve_context_window`).

**Credential precedence** — `get_api_key` in `crates/cli/src/cmd/helpers.rs`:
env var → config `api_key` → OAuth token store. Named credentials
(`[providers.<id>.credentials]` with `order`, `reserve`, `env`) are expanded
by `credential_candidates_for_lane`; the lane is `ci` when `CI` or
`WHYCODES_CREDENTIAL_LANE` says so (`process_is_ci_from`), else interactive.
The default env var name comes from `credential_env_var` (`<PROVIDER>_API_KEY`).
`openai` has one special case: an empty `OPENAI_API_KEY` still wins over a
missing config key. When the key came from the OAuth store, the provider is
registered with `oauth_refresh::register` so a 401 may trigger one forced
refresh and one retry (`send_with_refresh_retry`,
`crates/llm/src/oauth_refresh.rs`); an explicit key unregisters it.
`provider_requires_api_key` (`crates/llm/src/endpoint.rs`) skips the
credential check for `ollama` and for any provider whose `base_url` is a
loopback / RFC1918 / `.local` / `host.docker.internal` host.

**Which config fields reach a built-in provider.**
`ProviderRegistry::register_from_config` (`crates/llm/src/provider.rs`)
rebuilds only `ollama`, `anthropic` and `openai` from their `[providers.*]`
block (`from_config`). Every other built-in name is left as constructed by
`ProviderRegistry::default()`, so `base_url`, `headers`, `extra_body` and
`tool_arguments` under `[providers.xai]`, `[providers.deepseek]`, … are not
applied to requests (they still feed the `/v1/models` fetch). A name that
is not built in becomes a `CustomProvider`, which honours all of them.

**`extra_body`** — `ProviderConfig.extra` (`extra_body`, alias `extra`).
`apply_extra_body` (`crates/llm/src/openai_compat.rs`) merges the map into
the request body after the provider built it, so a key such as
`temperature`, `reasoning_effort` or `service_tier` overrides the computed
value; `model`, `messages`, `stream`, `tools` and `system` are protected and
logged as ignored. Applied by `AnthropicProvider`, `OpenAiProvider` and
`CustomProvider` only.

**`prompts/<provider>.md` overlays** — `Config::load_prompt_files`
(`crates/config/src/load.rs`) reads `<config dir>/prompts/` then
`<project>/.whycodes/prompts/` (project wins per key): `<provider>.md`,
`<provider>/<model>.md`, `models/<model>.md`. `Agent::with_prompt_overlays`
(`crates/agent/src/agent/mod.rs`) appends each as a `# <label>` section
after the role prompt; `system_prompt_for_route` rebuilds it on `/models`.

**`reasoning_effort`** — `[session] reasoning_effort` (`low` | `medium` |
`high` | `xhigh`). `ThinkingConfig::default_effort` (`crates/llm/src/thinking.rs`)
turns the field on for providers `openai`, `xai`, `openrouter`, `deepseek`
and for model families `o1*`, `o3*`, `o4*`, `gpt-5`, `grok-4`,
`grok-3-mini`, `*reasoner`, default `medium`. `xhigh` is accepted only for
Grok ≥ 4.6 (`model_allows_xhigh`); otherwise it clamps to `high`.
`apply_openai_effort` writes `reasoning_effort` into chat-completions
bodies; `apply_anthropic` writes `thinking: {type: enabled, budget_tokens}`
and raises `max_tokens` to at least `budget_tokens + 4096`.

**Model roles** — `[session] model_smol` (task and swarm workers),
`model_plan` (while `/agent plan`), `model_fast`, `title_model`, and the
first-token race partner `model_race` / `race_after_ms`
(`crates/config/src/types.rs`; `stream_raced` in `crates/llm/src/race.rs`).
Empty roles pick a small sibling of the session model.

**Pricing source** — nothing is bundled (`crates/llm/src/pricing.rs`). A
price is learned from an OpenRouter-style `pricing` object in `/v1/models`
during the context-window fetch (`price_for_model_id`), or set in
`[pricing."provider/model"]` (USD per million tokens; `price_overrides_from_config`
in `crates/agent/src/agent/mod.rs`). Config wins. OpenAI-style cached input
is inside `prompt_tokens` and is billed at the input rate, so `/cost` can run
high there; Anthropic cache fields are additive and priced separately.

**`fallback_models`** — `[session] fallback_models = ["provider/model", …]`.
`Agent` (`crates/agent/src/agent/mod.rs`) switches to the next entry when a
stream fails to open after retries (429 with credentials exhausted, 5xx,
network, timeout) and stays there for the rest of the turn; 400/401 never
fall back; entries without a resolvable key are skipped. Retries follow
`RetryPolicy` (`crates/llm/src/retry.rs`: 3 retries, 500 ms base, 20 s
cap, 60 s elapsed) and 401 is never retried there.

**Named credentials / lanes** — see precedence above; a 429 retries the
same model on the next name before fallback kicks in. `/cost` and
`whycodes stats` show the credential name only.

**Client identity** — `crates/llm/src/client_identity.rs`: one shared
`reqwest::Client` (5 s connect timeout, no request timeout, keep-alive,
`localhost` resolved IPv4-first), `User-Agent: whycodes/<version>`,
`X-Title: whycodes`, `HTTP-Referer: https://why.codes`. `post_for_provider`
lets a loaded auth plugin's `inference` object replace the User-Agent and
add headers; without a plugin the identity is always WhyCodes.

**Local servers** — `discover_local_servers` (`crates/llm/src/local_discovery.rs`)
probes `LOCAL_SERVER_CANDIDATES` on `127.0.0.1` concurrently: `lmstudio`
`:1234`, `vllm` `:8000`, `llamacpp` `:8080`, each a
`GET /v1/models` with one 250 ms connect+response budget
(`DEFAULT_DISCOVERY_TIMEOUT`). A 2xx JSON body with a `data` array counts as
found (`probe_openai_models`, model ids from `data[].id`); refused, timeout,
non-JSON and non-success replies are "not running" and debug-logged only.
It runs on demand (`whycodes provider discover`, the TUI provider dialog),
never at startup. `LocalServer::provider_config` is the block that gets
saved: `api_base`, no key, the listed `models`; each id becomes a
`CustomProvider`. `LocalServer::plan_save` keeps a configured block safe:
no block → `New`; a block without `base_url` / `api_base` → `FillApiBase`
(only that field changes); the same base (trailing `/` and case ignored) →
`AlreadyConfigured`, and the TUI drops that row from the dialog; any other
base → `Conflict`, nothing written. Ollama is not a candidate because it already has the
native `OllamaProvider`. `provider_requires_api_key` is false for all of
them because the host is loopback.

**Shared stream plumbing** — `line_buffer.rs` splits raw bytes on `\n`
before UTF-8 decoding so multi-byte characters survive chunk boundaries;
`think_tags.rs` turns a leading `<think>…</think>` in chat `content` into
thinking events; `usage_dump.rs` writes raw usage objects when
`WHYCODES_USAGE_DUMP` is set; `harmony.rs::uses_harmony_dialect` arms leak
guards for `gpt-5*`, `*codex*` and provider `openai-codex`, and every
provider escapes untrusted replay text with `whycodes_core::harmony::escape_replay`.

## Catalogue

### anthropic

`crates/llm/src/providers/anthropic.rs`, `AnthropicProvider`.

#### Special casing
Native Messages API at `normalize_anthropic_messages_url(base)`
(`crates/llm/src/endpoint.rs`: a bare host or `/v1` gains `/v1/messages`; a
chat-completions path is rewritten). `build_body` sets `max_tokens`
(default 4096), puts `system` top-level, converts tools to
`{name, description, input_schema}` without schema sanitising, applies
sampling, `ThinkingConfig::apply_anthropic`, then the prompt-cache policy
(`apply_anthropic_cache_policy`, `crates/llm/src/cache.rs`: breakpoints on
the last tool, the system block and the latest user message; at most four)
when `request.use_prompt_cache`, then `extra_body`. Assistant history drops
trailing thinking blocks (`strip_trailing_thinking`); non-assistant text and
tool results are `escape_replay`-ed; `System` and `Tool` roles become
`user`.

#### Stream behavior
`AnthropicSse` parses `data:` lines; `events_for_data` maps
`message_start` (input tokens, plus a separate `CacheUsage` event when
`cache_creation_input_tokens` / `cache_read_input_tokens` are non-zero),
`content_block_start` (`tool_use`, `thinking` + signature,
`redacted_thinking`), `content_block_delta` (`text_delta`,
`input_json_delta`, `thinking_delta`, `signature_delta`), `message_delta`
(stop reason; `usage` read as a sibling of `delta` or nested inside it, and
`output_tokens` is a running snapshot the agent folds with `max`),
`message_stop`, and `error` events as stream errors. `[DONE]` also stops.

#### Auth and usage
`authed_post`: a key starting with `sk-ant-oat` is sent as
`Authorization: Bearer` with `anthropic-beta: oauth-2025-04-20`; any other
key goes in `x-api-key`; an empty key sends neither (local proxies).
`anthropic-version: 2023-06-01`. OAuth tokens get the one-shot 401 refresh
via `send_with_refresh_retry`. Usage maps `input_tokens`, `output_tokens`
and both cache counters.

#### Catalog and model handling
No live catalogue (no `/v1/models` unless a `base_url` is configured).
`known_capabilities` marks `claude-sonnet-4` / `claude-opus-4` /
`claude-haiku-4` (200k, caching, thinking except haiku), `claude-3-7`,
`claude-3-5`, and the `claude-3-*` trio; heuristics give any `claude` model
200k. `caching` is true only for provider `anthropic`.

### antigravity

`crates/llm/src/providers/antigravity.rs`, `AntigravityProvider`
(`google-antigravity`); the transport lives in `codeassist.rs`.

#### Special casing
Always routes through `codeassist::complete_antigravity` /
`stream_antigravity`; there is no API-key path. `default_base_url` reports
`https://daily-cloudcode-pa.googleapis.com/v1internal`. `base_url` and
`extra_body` from config are not applied (not rebuilt by
`register_from_config`).

#### Stream behavior
Same Code Assist SSE as **codeassist** below.

#### Auth and usage
OAuth only (`whycodes auth login google-antigravity`, plugin required). The
bearer is sent through `post_for_provider`-style plugin identity
(`with_plugin_identity`), so a plugin may supply the hub User-Agent.

#### Catalog and model handling
`antigravity_wire_model` remaps picker ids: `gemini-3.1-pro` →
`gemini-3.1-pro-low`, `gemini-3.1-pro-high` → `gemini-pro-agent`,
`gemini-3.1-flash` / `gemini-3-flash` / `gemini-3.5-flash` →
`gemini-3.5-flash-low`, `gemini-3.6-flash` → `gemini-3.6-flash-low`,
`gemini-3.7-flash` → `gemini-3.7-flash-low`. `detect_capabilities` treats
`google-antigravity` as a tools-capable provider. Picker suggestions come
from the plugin's `suggested_models`.

### codeassist

`crates/llm/src/providers/codeassist.rs`. Not a registered provider name;
it is the backend for Google OAuth tokens (profile `GEMINI_CLI`, base
`https://cloudcode-pa.googleapis.com/v1internal`) and for Antigravity
(profile `ANTIGRAVITY`, bases `daily-cloudcode-pa` then its `sandbox` host).

#### Special casing
Requests are wrapped as `{model, project, request}`; the Antigravity
profile adds `userAgent: antigravity`, `requestType: agent`, a generated
`requestId`, `sessionId`, `labels` (`used_claude` when the model id contains
`claude`), `role: user` on `systemInstruction` and
`toolConfig.functionCallingConfig.mode = VALIDATED`. `build_inner_request`
maps tool use/result blocks to `functionCall` / `functionResponse` (names
recovered from earlier `ToolUse` ids), echoes thinking parts back as
`thought: true` with `thoughtSignature`, and sanitises tool schemas with
`sanitize_schema_for_openai` (Code Assist rejects `$schema` / `$id`).
`project_id` resolves the Cloud project: stored OAuth extra `project_id` →
`GOOGLE_CLOUD_PROJECT` (the Google OAuth profile defaults to `whycodes`) →
`:loadCodeAssist` → `:onboardUser` (polled up to 10 times, free tier unless
a project was supplied); cached process-wide per OAuth provider.
`post_generate` fails over to the next base on 404/408/429/5xx.

#### Stream behavior
`CodeAssistSse`: each `data:` chunk may be wrapped in `{"response": …}`;
`thought: true` parts become `Thinking`, `thoughtSignature` becomes
`ThinkingSignature`, `functionCall` parts get minted ids `gcall_<n>`
(Gemini sends none), `finishReason` becomes a `MessageDelta`, and
`usageMetadata` emits `Usage` + `MessageStop`. End of body without usage
still emits `MessageStop`.

#### Auth and usage
`Authorization: Bearer <ya29…>` with the one-shot 401 refresh keyed by the
profile's OAuth provider. Usage: `promptTokenCount` / `candidatesTokenCount`;
no cache fields.

#### Catalog and model handling
Error text is distilled by `code_assist_http_error` using
`error_class::extract_provider_reason`. No catalogue; capabilities come
from the `gemini*` rows in `capabilities.rs`.

### codex

`crates/llm/src/providers/codex.rs`. Not a registered provider name; the
Responses-API path `OpenAiProvider` delegates to for subscription tokens.

#### Special casing
`is_chatgpt_oauth_token` = starts with `eyJ` and has two dots (a JWT).
Posts to `https://chatgpt.com/backend-api/codex/responses` with
`store: false`, `stream: true`, the system prompt as `instructions`, tools
as flat `{type: function, name, description, parameters}` (no schema
sanitising), `tool_choice: auto`, `parallel_tool_calls: true`. History is
converted to `message` / `function_call` / `function_call_output` items;
thinking blocks are dropped; images are `input_image` data URLs for user
turns only. Headers: `OpenAI-Beta: responses=experimental`,
`Accept: text/event-stream`, and `chatgpt-account-id` from the stored OAuth
extra `openai_account_id`. `complete` is assembled from the same stream.

#### Stream behavior
`events_for_payload`: `response.output_text.delta` → text,
`response.output_item.done` with a `function_call` item → `ToolUse`,
`response.completed` → `Usage` + `MessageStop`, `response.failed` / `error`
→ stream error. Reasoning items are not surfaced. A closed connection
without `response.completed` still ends the turn with `MessageStop`.
Cached-token details are deliberately not mapped (they are a subset of
`input_tokens`).

#### Auth and usage
Bearer JWT via `send_with_refresh_retry("openai", …)`; identity via
`post_for_provider(url, "openai")` so an auth plugin may add an
`originator` header.

#### Catalog and model handling
No catalogue. `uses_harmony_dialect` is true for this path, so Harmony leak
guards are armed.

### copilot

`crates/llm/src/providers/copilot.rs`, `CopilotProvider` (`github-copilot`).

#### Special casing
Reuses `OpenAiProvider::new().build_body` (so no `extra_body`, no config
`base_url`) and posts to `https://api.githubcopilot.com/chat/completions`.

#### Stream behavior
Generic `chat_sse_stream(resp, "github-copilot")` with
`stream_options.include_usage`.

#### Auth and usage
`Authorization: Bearer <derived Copilot token>` through
`post_for_provider(url, "github-copilot")`; the token itself is produced by
the auth crate's device-code flow plus derived exchange
(`crates/auth/src/providers.rs::exchange_derived_token`), re-exchanged near
expiry and once more on a 401.

#### Catalog and model handling
No catalogue; capabilities fall through to the generic heuristics (the
provider name is not in the tools-capable list; whether any caller reads
that flag for this provider is not verified).

### custom

`crates/llm/src/providers/custom.rs`, `CustomProvider` — any
`[providers.<name>]` whose name is not built in.

#### Special casing
`from_config`: `base_url` / `api_base` (default `https://api.openai.com/v1`)
normalised by `normalize_chat_completions_url`; config `headers` are sent
as-is and may override the identity headers; `Authorization: Bearer
<api_key>` is added only when no `Authorization` header is configured;
`tool_arguments = "object"` switches assistant `function.arguments` from a
JSON string to a bare object (`convert_messages_with_format`); `extra_body`
is applied. The `api_key` argument passed by the agent is ignored — the
credential must be in the config block or headers.

#### Stream behavior
Generic `chat_sse_stream`. Non-2xx on open includes the numeric status so
`retry_with_backoff` can see a 5xx.

#### Auth and usage
Config only (see above). Usage from `prompt_tokens` / `completion_tokens`.

#### Catalog and model handling
`/v1/models` works whenever `base_url` is set; `is_local_llm_endpoint`
waives the credential check for local hosts. `uses_harmony_dialect`
applies to provider `custom` for `gpt-5` / `codex` ids.

### deepseek

`crates/llm/src/providers/deepseek.rs`, `DeepSeekProvider`.

#### Special casing
`https://api.deepseek.com/v1/chat/completions`; plain
`chat_completions_body`. `replays_reasoning` is false for
`deepseek-reasoner` / `deepseek-r1`, so past `reasoning_content` is
stripped before sending (other DeepSeek ids replay it).

#### Stream behavior
Generic; `reasoning_content` deltas become thinking events.

#### Auth and usage
`Authorization: Bearer`; no OAuth, no refresh retry.

#### Catalog and model handling
`known_capabilities`: `deepseek-r1` (no tools, thinking, 128k),
`deepseek-v3` / `deepseek-chat` (128k); other `deepseek*` ids get 64k.
`default_effort` is on for the whole provider.

### google

`crates/llm/src/providers/google.rs`, `GoogleProvider`.

#### Special casing
API-key route: `generativelanguage.googleapis.com/v1beta/models/<model>:
generateContent|streamGenerateContent?alt=sse&key=<key>` — the key travels
in the URL. `build_body` sends only text: every message is flattened with
`MessageContent::as_text()` and non-text becomes `[non-text content]`, so
tool calls and results are not round-tripped on this path (the OAuth
Code Assist path does). Tools are `functionDeclarations` with sanitised
schemas; `maxOutputTokens` and `temperature` in `generationConfig`.
`is_google_oauth_token` (`ya29.` prefix) redirects to **codeassist**.

#### Stream behavior
The whole response body is read first (`resp.text()`), then
`events_from_google_stream_body` splits the JSON array by `}\n,{`
markers and replays text deltas, `finishReason` and `usageMetadata`
(`Usage` + `MessageStop`). Function calls in the stream are not emitted.
There is no true incremental streaming on the API-key path.

#### Auth and usage
`AIza…` API key in the query string; OAuth tokens never reach this module's
HTTP code. Usage from `promptTokenCount` / `candidatesTokenCount`.

#### Catalog and model handling
`known_capabilities`: `gemini-2.5-pro|flash` and `gemini-2.0-flash` (1M,
thinking for 2.5), `gemini-1.5-pro` (2M), `gemini-1.5-flash` (1M); other
`gemini` ids 128k. Provider ids `google` and `gemini` are tools-capable.

### groq

`crates/llm/src/providers/groq.rs`, `GroqProvider`.

#### Special casing
`https://api.groq.com/openai/v1/chat/completions`; plain
`chat_completions_body`; no `extra_body`.

#### Stream behavior
Generic.

#### Auth and usage
`Authorization: Bearer`.

#### Catalog and model handling
No family rows in `capabilities.rs`; unknown ids get the 128k default.
`default_effort` is off unless the model id matches a reasoning family.

### mistral

`crates/llm/src/providers/mistral.rs`, `MistralProvider`.

#### Special casing
`https://api.mistral.ai/v1/chat/completions`. `NO_REASONING_REPLAY` lists
`mistral`, `mixtral`, `codestral`, `devstral`, `magistral`, `pixtral`,
`ministral`: `reasoning_content` is dropped from history because the API
answers 422 otherwise, and an assistant turn left empty by that is removed.

#### Stream behavior
Generic.

#### Auth and usage
`Authorization: Bearer`.

#### Catalog and model handling
`known_capabilities`: `mistral-large` / `pixtral` (128k, vision for
pixtral), `codestral` / `mistral-small` (32k); heuristics give
`mistral` / `mixtral` 32k.

### ollama

`crates/llm/src/providers/ollama.rs`, `OllamaProvider`.

#### Special casing
Native `POST {host}/api/chat`, not OpenAI SSE. `normalize_ollama_chat_url`
accepts a bare host, `http://host:port`, `/api/chat`, or an OpenAI-style
`/v1` URL (stripped back), from config `base_url` / `api_base` or
`OLLAMA_HOST`, default `http://localhost:11434`. The system prompt is a
`system` message; `temperature`, `num_predict` (from `max_tokens`) and
`top_p` sit under `options`; messages are flattened to text, with base64
images attached as `images`; tools are OpenAI-shaped with sanitised schemas.
Rebuilt from config by `register_from_config`.

#### Stream behavior
`OllamaNdjson`: one JSON object per line; `message.content` → text,
`message.tool_calls` → `ToolUse` (arguments may be an object or a string),
`done: true` with `prompt_eval_count` / `eval_count` → `Usage` then
`MessageStop`; an `error` field becomes a stream error. A leftover
unterminated line is parsed at EOF.

#### Auth and usage
Optional `Authorization: Bearer` when a key is non-empty;
`provider_requires_api_key` is always false for `ollama`.

#### Catalog and model handling
`/v1/models` is attempted only with a configured `base_url` (whether the
reply carries a context length is not verified). Generic heuristics apply.

### openai

`crates/llm/src/providers/openai.rs`, `OpenAiProvider`.

#### Special casing
`https://api.openai.com/v1/chat/completions` or a normalised config
`base_url`; `chat_completions_body` plus `extra_body`. If the credential is
a ChatGPT-subscription JWT (`codex::is_chatgpt_oauth_token`) the whole call
is delegated to **codex**. `uses_harmony_dialect` for `gpt-5*` / `*codex*`.

#### Stream behavior
Generic `chat_sse_stream(resp, "openai")`; `stream_options.include_usage`
is attached so the final usage-only chunk is parsed
(`stream_usage_from_chunk`). Tool-call deltas keyed by `index` until an id
arrives (`stream_events_for_tool_call_delta`).

#### Auth and usage
`Authorization: Bearer` for API keys; empty key sends no header. The
API-key path has no 401 refresh retry (only the Codex path does).

#### Catalog and model handling
`known_capabilities` rows for `gpt-4o*`, `gpt-4.5*`, `gpt-5*` (200k,
thinking), `gpt-4-turbo` / `gpt-4-*`, `gpt-3.5*`, `o1*`, `o3*` / `o4*`.
`default_effort` is on for the whole provider.

### openrouter

`crates/llm/src/providers/openrouter.rs`, `OpenRouterProvider`.

#### Special casing
`https://openrouter.ai/api/v1/chat/completions`; sends `HTTP-Referer` and
`X-Title` from `with_site` (defaults to the WhyCodes identity). No
`extra_body`, no config `base_url` (not rebuilt from config). Default
provider for a fresh config (`DEFAULT_PROVIDER = "openrouter"`,
`DEFAULT_MODEL_ID = "anthropic/claude-sonnet-4.5"` in
`crates/config/src/types.rs`).

#### Stream behavior
Generic; `reasoning` (string or `{content}`) deltas become thinking events.

#### Auth and usage
`Authorization: Bearer`.

#### Catalog and model handling
The `/v1/models` body is also mined for `pricing` (`price_from_model_value`:
USD per token, often strings) and for `top_provider.context_length` /
`architecture.context_length`; a `provider/model` id matches by suffix.
`default_effort` is on for the provider; `uses_harmony_dialect` for
`gpt-5` / `codex` ids.

### together

`crates/llm/src/providers/together.rs`, `TogetherProvider`.

#### Special casing
`https://api.together.xyz/v1/chat/completions`; plain body; no `extra_body`.

#### Stream behavior
Generic.

#### Auth and usage
`Authorization: Bearer`.

#### Catalog and model handling
No family rows; 128k default. `uses_harmony_dialect` only for `*codex*`
ids.

### xai

`crates/llm/src/providers/xai.rs`, `XaiProvider`.

#### Special casing
Two endpoints chosen per credential by `inference_url`: console keys
(`xai-…`) go to `https://api.x.ai/v1/chat/completions`; anything else is
treated as a SuperGrok / X Premium OAuth token (`is_xai_oauth_token`) and
goes to `https://cli-chat-proxy.grok.com/v1/chat/completions` with plugin
identity headers (`post_for_provider`). `convert_one_message` drops a
`tool` message with an empty `tool_call_id` because the API rejects it.

#### Stream behavior
Generic; `reasoning_content` / `reasoning` deltas become thinking events.

#### Auth and usage
`Authorization: Bearer`; the one-shot 401 refresh retry applies when the
key came from the OAuth store.

#### Catalog and model handling
`known_capabilities`: `grok-4` (256k, vision, thinking), `grok-3`
(131k, thinking for `mini` / `reason`), `grok-2`, other `grok`. Provider
`xai` is tools-capable with a 131k heuristic. `default_effort` is on and
`xhigh` is allowed from `grok-4.6` (`grok_version_at_least`).

## Adding a provider

1. **Module** — add `crates/llm/src/providers/<name>.rs` with a struct
   implementing `LlmProvider` (`crates/llm/src/provider.rs`): `name`,
   `default_base_url`, `complete`, `stream`. For an OpenAI-compatible API,
   build on `openai_compat::chat_completions_body`, `convert_messages`
   (or `convert_messages_with_format`), `content_blocks_from_chat_message`,
   `usage_from_chat_completion`, `attach_stream_usage_option` and
   `chat_sse_stream`; post through `client_identity::post` (or
   `post_for_provider` when a plugin identity applies). Keep a `from_base`
   constructor and run it through `custom::normalize_chat_completions_url`.
   For a native wire format, write a `Stream` over
   `openai_compat::response_bytes` with `line_buffer::LineBuffer`, as
   `anthropic.rs`, `ollama.rs` and `codeassist.rs` do.
2. **Register** — `pub mod <name>` in `providers/mod.rs`, re-export in
   `lib.rs`, and `registry.register(Box::new(...))` in
   `ProviderRegistry::default()`. If config `base_url` / `headers` /
   `extra_body` must apply, add a `from_config` constructor and an arm in
   `register_from_config`; otherwise the config block only supplies the key.
3. **Credential** — `<PROVIDER>_API_KEY` works automatically through
   `credential_env_var`. A credential that must bypass the key check (local
   host) is covered by `endpoint::provider_requires_api_key`. OAuth is an
   auth plugin plus `oauth_refresh::send_with_refresh_retry` around the
   POST; document it in `docs/auth.md`.
4. **Capabilities** — add family rows to `known_capabilities` and, if
   needed, `heuristic_context_window` and the tools-capable provider list in
   `detect_capabilities` (`crates/llm/src/capabilities.rs`). Add the
   provider or model family to `ThinkingConfig::default_effort` when it
   accepts `reasoning_effort`, to `NO_REASONING_REPLAY` when it rejects
   replayed `reasoning_content`, and to `uses_harmony_dialect` when it
   speaks Harmony.
5. **Catalogue and pricing** — nothing to register: `/v1/models` is used
   when the config block has a `base_url`, and prices come from the
   catalogue or `[pricing]`. If the API reports context length under a new
   key, extend `context_window_from_model_value`.
6. **Tests** — add `providers/<name>_tests.rs` (wired with
   `#[path = "<name>_tests.rs"]`), extend
   `providers/mod_tests.rs::openai_compat_wrappers_build_body_and_name_their_host`
   for the body and host, `provider_tests.rs` for the registry name,
   `capabilities_tests.rs` / `thinking_tests.rs` for new rows, and the
   `harmony.rs` inline tests. Run
   `python scripts/affected_tests.py --paths crates/llm/src/providers/<name>.rs --execute`
   and keep the crate's coverage floor (`docs/coverage.md`).
7. **Docs** — add the `### <name>` section here and, when user-facing
   behaviour changes, the `docs/guide.md` configuration section.

## Related

- [`auth.md`](auth.md) — credential kinds, OAuth plugins, import, the
  per-provider flow table.
- [`guide.md`](guide.md) — `[providers.*]`, `extra_body`, model roles,
  `reasoning_effort`, prompt overlays, named credentials, `fallback_models`,
  `/cost`.
- [`knowhow.md`](knowhow.md) — hard rules 5 and 6 (restated above).
- [`architecture.md`](architecture.md) — `llm` and `auth` crate boundaries.
- `crates/llm/src/error_class.rs` — error classification shared by every
  provider (`ErrorKind`, `extract_provider_reason`).
