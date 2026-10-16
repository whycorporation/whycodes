# MCP lifecycle

> Status: Implemented · Updated: 2026-10-16

WhyCodes is both an MCP client and a minimal MCP server. As a client it
reads `[mcp_servers.<name>]` blocks from `config.toml`, connects every server
once at startup (stdio, Streamable HTTP, legacy HTTP+SSE, or auto-probe),
lists each server's tools and registers them on the agent's `ToolExecutor`
as `{server}_{tool}`. Servers that advertise resources add two shared tools
(`mcp_list_resources`, `mcp_read_resource`); servers that advertise prompts
become TUI slash commands (`/mcp__<server>__<prompt>`). Remote servers can be
authorized with `whycodes mcp login <name>` (OAuth with dynamic client
registration); the token lives apart from provider logins. There is no
reconnect, no `list_changed` handling and no per-session tool cache: what a
server reports at startup is what the session has.

This page follows the code; `docs/guide.md` ("Tools", "Configuration") is the
user-facing summary.

## How it works

### 1. Configuration

`McpServerConfig` (`crates/config/src/types.rs`) is one `[mcp_servers.<name>]`
table. `resolved_transport()` decides the transport: an explicit `type` is
validated (`stdio` needs `command`, `http`/`sse`/`auto` need `url`); with no
`type`, a `url` means `auto` and a `command` means `stdio`; neither is a
config error. `is_remote()` is true for the three URL transports and gates
the OAuth header below.

`whycodes mcp add|list|remove` (`cmd_mcp` in `crates/cli/src/cmd/mcp.rs`)
edit that table through `Config::load_or_create` / `Config::save`.
`parse_mcp_transport` accepts the aliases `local` (stdio), `streamable-http`
and `remote` (http); `parse_mcp_headers` takes `--header "Key: Value"`.
`whycodes import` also copies MCP entries from other agents' settings files
(`mcp_from_map` in `crates/import/src/extract.rs`).

### 2. Connect

`register_mcp_tools` (`crates/agent/src/mcp_load.rs`) is called from
`Agent::load_mcp` / `Agent::with_mcp` (`crates/agent/src/agent/mod.rs`). For
every configured server it:

1. Runs `with_oauth_header`: for remote servers only, if no `Authorization`
   header (case-insensitive) is configured, `whycodes_mcp::oauth::bearer`
   reads `<data_dir>/mcp/auth.json` under `mcp:<server>` and adds
   `Authorization: Bearer …`, refreshing an expired token first.
2. Calls `connect_mcp_server`, which maps the transport to
   `McpClient::connect_stdio_with` / `connect_http` / `connect_sse` /
   `connect_auto` (`crates/mcp/src/client.rs`).

All connect futures run under `futures::future::join_all`, so a cold start
with N servers costs the slowest connect, not the sum. `tools/list` is then
run per server, sequentially, after the join.

Transport details:

- **stdio** — `tokio::process::Command` with piped stdin/stdout, inherited
  stderr and `kill_on_drop(true)`. Before the configured `env` is applied,
  `whycodes_core::secret_env::strip_secret_env_vars` removes known secrets
  (`*_API_KEY`, `GITHUB_TOKEN`, `WHYCODES_*_TOKEN|_KEY|_SECRET`, …) so a
  server never inherits provider credentials unless `env` re-adds them.
  `cwd` is honoured. Requests are one JSON line each; `read_stdio_response`
  reads exactly one line back and has no timeout.
- **http** (Streamable HTTP, `StreamableHttpTransport` in
  `crates/mcp/src/http.rs`) — one `POST` per request with
  `Accept: application/json, text/event-stream`; a `mcp-session-id`
  response header is captured and echoed on later requests. A JSON body is
  parsed directly; an SSE body is read to completion and the matching
  JSON-RPC response is extracted (`extract_jsonrpc_result_from_sse`).
  Notifications expect 202 or any 2xx.
- **sse** (legacy HTTP+SSE, `LegacySseTransport`) — a `GET` with
  `Accept: text/event-stream` opens a background reader task; the first
  `endpoint` event (or a bare data line that looks like a URL) is resolved
  against the SSE URL by `resolve_endpoint_url`. Requests `POST` to that
  endpoint; a 202/204 (or an empty JSON body) means the answer arrives on the
  SSE stream, where `wait_for_response_until` skips server notifications and
  responses with another id.
- **auto** — `connect_auto` tries Streamable HTTP; only when the error text
  contains `404`, `405`, `400`, `Method Not Allowed` or `Not Found` does it
  fall back to legacy SSE. Any other failure (TLS, timeout, 401, 500) is
  returned as-is.

Both HTTP transports share `http_client()`: 60 s request timeout, 15 s
connect timeout, `User-Agent: whycodes-mcp/<version>`. Legacy SSE waits 15 s
for the `endpoint` event and 60 s for each response on the stream.

### 3. Handshake

`initialize_handshake` sends `initialize` with
`protocolVersion = "2025-03-26"`, `clientInfo = whycodes/<version>` and
empty client capabilities, then `notifications/initialized`. The server's
`capabilities` are kept on the client; `supports_prompts()` /
`supports_resources()` read them later. The server's reported protocol
version is logged, not negotiated.

### 4. list_tools and name binding

After a successful connect, `McpClient::list_tools` (`tools/list`, first
page only) returns `McpTool { name, description, inputSchema }`. If the
listing fails the server is logged with a warning and dropped entirely (no
handle, no tools). For each tool `register_mcp_tools` builds an
`McpToolBridge` (`crates/tools/src/mcp.rs`) with:

- name from `bridge_tool_name(server, tool)`: `{server}_{tool}`, every
  character outside `[A-Za-z0-9_-]` replaced by `_`; names longer than 64
  characters keep a 55-character prefix plus `_` and an 8-hex FNV-1a hash of
  the raw name, so the result is stable across sessions and distinct for
  long siblings;
- description, or `MCP tool from '<server>'` when the server sent none;
- the server's `inputSchema`, or `{"type":"object","properties":{}}` when it
  was `null`.

`ToolExecutor::register` (`crates/tools/src/executor.rs`) inserts by name
into a map and clears the definitions cache. Built-ins are registered first,
then shell plugins, then MCP tools, so an MCP name that equals an existing
one silently replaces it.

### 5. Resources and prompts

`register_resource_tools` (`crates/agent/src/mcp_content.rs`) runs once
after all servers are up. If at least one connected server advertised the
`resources` capability, two bridges are registered:

- `mcp_list_resources` (optional `server`) calls `resources/list` on each
  qualifying server and returns `[server] name <uri> — description` lines
  (per-server errors are inlined, not fatal);
- `mcp_read_resource` (`server`, `uri`) calls `resources/read`; text comes
  back verbatim, a blob is summarised as
  `[binary <mime> at <uri>, N base64 chars]`.

`read mcp://<server>/<uri>` is the same read through the `read` tool:
`dispatch_tool` answers it in-process (`execute_mcp_read` →
`read_mcp_url`) because the clients live on the agent, not in the tool
executor. `mcp://<server>` lists that server's resources (an error when it
advertises none) and `mcp://` the connected servers, whether or not the
resource tools were registered.

Prompts are not registered as tools. In the TUI, `/mcp__<server>__<prompt>
args` reaches `render_mcp_prompt`: it calls `prompts/list` on that server
every time, fills arguments positionally (the last declared argument takes
the rest of the line), rejects a missing `required` argument with a usage
line, then calls `prompts/get` and sends `GetPromptResult::text()` (message
texts joined by blank lines; embedded resources contribute their text,
images are dropped) as the user message. `/mcp` prints
`describe_mcp_servers`: each server with its transport, whether resources
are available, and its prompts with argument names
(`crates/tui/src/run/slash.rs`). The plain REPL has no MCP prompt commands.

### 6. Executing a bound tool

A model call to `{server}_{tool}` goes through the normal gate in
`Agent::execute_with_permission` (`crates/agent/src/agent/gate.rs`):

1. **Visibility.** The default `session.tool_profile = "core"` advertises
   only `CORE_TOOL_NAMES` (`crates/tools/src/profile.rs`). MCP bridges and
   the two resource tools are not in that list, so they sit in the deferred
   catalogue (`ToolExecutor::deferred_catalog`) and reach the model after a
   `tool_search` activation, or always under `tool_profile = "full"`.
2. **Permission key** is the bound name. `PermissionSet::action_for`
   (`crates/core/src/types.rs`) checks an exact `[permission]` rule, then
   prefix globs such as `"mymcp_*" = "deny"`, then the legacy allow/deny
   lists. No category flag (`allow_network`, `allow_shell`,
   `allow_file_writes`) covers MCP names, so with no rule the answer is
   `Allow` and the call runs without a prompt.
3. **Shell risk, intent and path gates** apply only to shell tools and to
   tools whose arguments carry a path the gate recognises; an MCP tool with
   arbitrary JSON arguments normally hits none of them.
4. **Hooks**: pre-tool hooks may block; post-tool hooks may append feedback.
5. **Dispatch**: `McpToolBridge::execute` ignores `ToolContext` entirely
   (no cwd, sandbox, network policy or file claims), locks the shared
   `Arc<Mutex<McpClient>>` and calls `tools/call` with the original remote
   name. The mutex serialises calls per server; calls to different servers
   can overlap. `CallToolResult::text()` joins text blocks with newlines;
   image and embedded-resource blocks are dropped. A server-side
   `isError: true` is only logged — the bridge still returns
   `is_error: false`. Transport failures become
   `MCP tool '<name>' error: …` with `is_error: true`.

MCP names are not in `SERIAL_TOOLS` (`crates/agent/src/tool_policy.rs`),
so a batch of MCP calls is executed in parallel with other parallel-safe
tools. The OS sandbox and the HTTP network allowlist never see MCP traffic:
a stdio server was spawned outside the sandbox at startup, and remote
servers are plain `reqwest` calls from the agent process.

### 7. Failure and reconnection

- A connect failure is logged once with `command=…` or `url=…` and the
  error; if the error text contains `401`, `login_hint` adds
  `run \`whycodes mcp login <server>\``. The server is skipped for the whole
  session.
- A failed `tools/list` drops the server the same way.
- After startup there is no health check and no reconnect. A stdio server
  that exits makes every later call fail with `MCP server closed stdout`;
  a stdio server that writes a notification (a JSON-RPC message without
  `id`) on stdout makes the pending request fail with a parse error. A
  legacy-SSE stream that closes surfaces as `SSE stream closed while
  waiting for response`. The tool stays registered and keeps returning
  errors; `ApprovalMode::Auto` may retry it a few times like any other tool.
- `ping` exists on the client but nothing calls it in the agent.

### 8. Shutdown

`McpClient` has an empty `Drop`. For stdio, `kill_on_drop(true)` on the
child reaps the process when the last `Arc<Mutex<McpClient>>` is dropped;
for HTTP there is no `DELETE` of the `mcp-session-id` and no explicit close.
TUI sessions in one process share a single executor and server set
(`Agent::shared_tools` / `with_shared_tools`), so servers start once per
process, not per session.

### 9. Startup gating

- TUI: `hydrate_after_first_frame` (`crates/tui/src/run/mod.rs`) awaits
  `rt.agent.load_mcp(config)` after the first frame is painted and before
  the `'main` event loop starts, so the prompt is not interactive until
  every server has connected or failed. The sidebar MCP tab lists the
  configured server names from `config.mcp_servers`, not connection state.
- Plain REPL, one-shot `run` and `serve`: `Agent::with_mcp` is awaited
  before the session is created (`crates/cli/src/cmd/run.rs`,
  `crates/cli/src/cmd/serve.rs`).

### 10. WhyCodes as a server

`whycodes mcp serve [--tools core|full] [--cwd …]` runs
`whycodes_mcp::run_stdio_server` (`crates/mcp/src/server.rs`): protocol
`2024-11-05`, methods `initialize`, `tools/list`, `tools/call`, `ping`;
prompts and resources are not advertised. Tools execute with
`allow_file_writes`, `allow_network` and `allow_shell` all true, an
unrestricted `NetworkPolicy` and default sandbox settings — the host that
connects is trusted.

## Configuration and overrides

| Key / flag | Where | Effect |
|---|---|---|
| `[mcp_servers.<name>] type` | `McpServerConfig.transport` | `stdio` \| `http` \| `sse` \| `auto`; validated against `command` / `url` |
| `command`, `args`, `env`, `cwd` | stdio | Spawned process; `env` is applied after secret stripping |
| `url` | http / sse / auto | Endpoint (Streamable HTTP) or SSE URL |
| `headers` | remote | Extra request headers; an `Authorization` entry wins over a stored OAuth token |
| `whycodes mcp add <name> <command> [--args "…"]` | CLI | Writes a stdio entry |
| `whycodes mcp add <name> --url … [--type …] [--header "K: V"]` | CLI | Writes a remote entry |
| `whycodes mcp login|logout <name>` | CLI | OAuth token under `<data_dir>/mcp/auth.json`, key `mcp:<name>` |
| `whycodes mcp serve --tools --cwd` | CLI | Export WhyCodes tools over stdio |
| `[permission] "<server>_*" = allow|ask|deny` | `PermissionSet.rules` | Per-server or per-tool policy; default is allow |
| `[session] tool_profile = "full"` | `Agent.tool_profile` | Advertise MCP tools without `tool_search` |
| Custom subagent `tools: [mcp__<server>__<tool>]` | `custom_agents.rs::map_tool` | Mapped to `<server>_<tool>` verbatim (no sanitising or hashing) |
| Connect timeout 15 s, request timeout 60 s | `http.rs::http_client` | Remote transports only; not configurable |
| SSE endpoint wait 15 s, SSE response wait 60 s | `http.rs` | Legacy SSE only; not configurable |
| OAuth callback wait 300 s | `oauth.rs::CALLBACK_TIMEOUT` | `mcp login` browser step |

## Implementation files

- `crates/config/src/types.rs` — `McpServerConfig`, `McpTransportKind`,
  `resolved_transport`, `is_remote`.
- `crates/mcp/src/client.rs` — `McpClient`: connect variants, handshake,
  `list_tools`, `call_tool`, `list_resources`, `read_resource`,
  `list_prompts`, `get_prompt`, `ping`.
- `crates/mcp/src/http.rs` — `StreamableHttpTransport`,
  `LegacySseTransport`, timeouts, `resolve_endpoint_url`.
- `crates/mcp/src/sse.rs` — incremental SSE parser.
- `crates/mcp/src/oauth.rs` — `discover`, `login`, `bearer`, `refresh`,
  `logout`; token store under `<data_dir>/mcp`.
- `crates/mcp/src/types.rs` — JSON-RPC and MCP payload types
  (`CallToolResult::text`, `GetPromptResult::text`).
- `crates/mcp/src/server.rs` — `run_stdio_server`, `handle_rpc`.
- `crates/agent/src/mcp_load.rs` — `register_mcp_tools`,
  `connect_mcp_server`, `with_oauth_header`, `login_hint`,
  `bridge_tool_name`, `McpServerHandle`.
- `crates/agent/src/mcp_content.rs` — `register_resource_tools`,
  `render_mcp_prompt`, `describe_mcp_servers`.
- `crates/agent/src/agent/mod.rs` — `load_mcp`, `with_mcp`, `shared_tools`.
- `crates/agent/src/agent/gate.rs`, `crates/agent/src/tool_policy.rs` —
  permission / hook gate and the parallel-safe set.
- `crates/tools/src/mcp.rs` — `McpCaller`, `McpToolBridge`.
- `crates/tools/src/executor.rs`, `crates/tools/src/profile.rs` —
  registration, definitions cache, core profile.
- `crates/cli/src/cmd/mcp.rs` — `whycodes mcp` subcommands.
- `crates/tui/src/run/slash.rs`, `crates/tui/src/run/mod.rs` — `/mcp`,
  prompt commands, deferred load after first frame.
- `crates/core/src/secret_env.rs` — credentials stripped from children.

## Known limitations / not enforced

- **Startup waits for every server.** Connects run concurrently, but the
  TUI's first interactive frame and the plain REPL's session creation wait
  for the whole `join_all`. A hanging stdio server has no connect timeout at
  all (the handshake read has none); a remote one is bounded by 15 s + 60 s.
- **Tool schemas are not cached across runs.** Every process start runs
  `initialize` + `tools/list` for each server; only the first `tools/list`
  page is read (`nextCursor` is ignored).
- **No mid-session changes.** Client capabilities are sent empty and
  `notifications/tools/list_changed` is never handled; a server cannot add,
  remove or rename tools after startup, so the tool definitions (and thus
  the prompt/tool prefix sent to the model) are stable for the session. A
  flapping server only makes calls fail.
- **No reconnect policy.** A failed connect, a dead child, or a closed SSE
  stream is permanent until the next process start.
- **Name collisions replace silently.** `ToolExecutor::register` keeps the
  last registration: a server named `git` with a tool `status` replaces the
  built-in `git_status`; two servers can collide after sanitising (server
  `a`, tool `b.c` and server `a_b`, tool `c` both bind as `a_b_c`).
- **Long names are hashed, but only in the bridge.** `bridge_tool_name`
  truncates past 64 characters; the custom-agent `mcp__server__tool`
  mapping in `custom_agents.rs::map_tool` does not sanitise or hash, so such
  entries never match.
- **Deferred by default.** Under the default `core` tool profile, MCP tools
  are visible to the model only after `tool_search` activates them.
- **Permission default is allow**, with no sandbox, network allowlist or
  risk gate; mutating MCP tools also run in parallel with other calls.
- **Result fidelity.** `isError: true` from a server is reported to the model
  as a successful result; image and embedded-resource content blocks are
  dropped from tool results and prompts.
- **stdio stderr is inherited** by the WhyCodes process; a chatty server
  writes into the terminal (under the TUI this is not verified to be
  harmless).
- **Streamable HTTP is request/response only.** No server-initiated `GET`
  stream is opened and the session id is never deleted.
- **OAuth requires dynamic client registration** (`registration_endpoint`);
  servers without it need a static `Authorization` header. Scopes come from
  discovery only, and a freshly stored token is used from the next session.
- **Protocol versions are pinned**: the client announces `2025-03-26`, the
  server mode `2024-11-05`; neither negotiates.

## Related

- `docs/guide.md` — "Tools" (MCP binding, resources, prompts), the
  `[mcp_servers]` config example and the "Remote MCP servers that use OAuth"
  paragraph.
- `docs/auth.md` — provider OAuth (separate store from MCP tokens) and
  `whycodes import`.
- `docs/knowhow.md` — "2026-09-25 — Windows coverage: MCP stdio tests spawn
  0 tools".
- `docs/architecture.md` — crate map (`mcp` is a service crate; `agent`
  owns loading).
