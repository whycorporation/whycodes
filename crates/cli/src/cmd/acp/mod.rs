//! `whycodes acp` — Agent Client Protocol over stdio (Zed, Neovim, Emacs, …).
//!
//! The editor spawns `whycodes acp` and speaks JSON-RPC 2.0, one message per
//! line. Each `session/new` builds an agent for its `cwd`; `session/prompt`
//! runs a turn and streams it back as `session/update` notifications. Tool
//! approvals go to the editor as `session/request_permission`. Sessions are
//! saved like any other, so `session/load` (and `whycodes --resume`) work.

mod map;
mod wire;

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use whycodes_agent::permission::PermissionAskFuture;
use whycodes_agent::{
    Agent, AutoAnswerPrompter, CancelFlag, PermissionPrompter, TurnEvent, TurnOpts,
    new_cancel_flag, request_cancel,
};
use whycodes_config::Config;
use whycodes_session::session::Session;

use crate::Cli;
use crate::cmd::helpers::{
    agent_info_for, apply_sticky_credential, expand_user_input, get_api_key, maybe_inject_test_llm,
    missing_api_key_message_for, open_db, resolve_model, resolve_provider, with_project_memory,
};
use wire::{INTERNAL_ERROR, INVALID_PARAMS, METHOD_NOT_FOUND, Outbox, PARSE_ERROR};

/// ACP protocol version spoken here.
const PROTOCOL_VERSION: u64 = 1;

/// How long a permission request waits for the matching `tool_call` update.
const TOOL_CALL_WAIT: Duration = Duration::from_millis(500);

pub(crate) async fn cmd_acp(cli: &Cli) -> anyhow::Result<()> {
    let input = tokio::io::BufReader::new(tokio::io::stdin());
    serve(input, tokio::io::stdout(), CliFactory { cli }).await
}

/// Provider route for a session's turns.
#[derive(Clone)]
pub(crate) struct Route {
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) api_key: String,
}

/// An agent and session ready for prompts (permission prompter not yet set).
pub(crate) struct Built {
    pub(crate) agent: Agent,
    pub(crate) session: Session,
    pub(crate) route: Route,
}

/// Builds agents per `cwd`. The CLI resolves config and credentials; tests
/// script the provider.
pub(crate) trait Factory: Sync {
    fn build(
        &self,
        cwd: &Path,
        resume: Option<&str>,
    ) -> impl Future<Output = anyhow::Result<Built>> + Send;
}

/// `(tool name, tool call id)` for calls started but not finished.
type OpenTools = Arc<Mutex<Vec<(String, String)>>>;

struct Slot {
    agent: Agent,
    session: tokio::sync::Mutex<Session>,
    route: Route,
    cwd: PathBuf,
    cancel: Mutex<Option<CancelFlag>>,
    open_tools: OpenTools,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

type RpcResult = Result<Value, (i64, String)>;

struct Server<F> {
    factory: F,
    out: Outbox,
    sessions: HashMap<String, Arc<Slot>>,
    tasks: JoinSet<()>,
}

/// Serve ACP until `input` closes.
pub(crate) async fn serve<R, W, F>(input: R, output: W, factory: F) -> anyhow::Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
    F: Factory,
{
    let (out, rx) = Outbox::new();
    let writer = tokio::spawn(wire::write_loop(output, rx));
    let mut server = Server {
        factory,
        out: out.clone(),
        sessions: HashMap::new(),
        tasks: JoinSet::new(),
    };
    let mut lines = input.lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(&line) {
            Ok(msg) => server.handle(msg).await,
            Err(e) => out.respond_error(Value::Null, PARSE_ERROR, &format!("invalid JSON: {e}")),
        }
    }
    server.shutdown().await;
    drop(server);
    drop(out);
    writer.await??;
    Ok(())
}

impl<F: Factory> Server<F> {
    async fn handle(&mut self, msg: Value) {
        while self.tasks.try_join_next().is_some() {}
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            self.out.resolve(&msg);
            return;
        };
        let method = method.to_string();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        match msg.get("id").cloned() {
            Some(id) => self.request(id, &method, params).await,
            None => self.notification(&method, &params),
        }
    }

    fn notification(&self, method: &str, params: &Value) {
        if method == "session/cancel"
            && let Some(slot) = self.slot(params)
            && let Some(flag) = lock(&slot.cancel).as_ref()
        {
            request_cancel(flag);
        }
    }

    async fn request(&mut self, id: Value, method: &str, params: Value) {
        let result = match method {
            "initialize" => Ok(initialize_result()),
            "authenticate" => Ok(json!({})),
            "session/new" => self.open_session(&params, None).await,
            "session/load" => match params.get("sessionId").and_then(Value::as_str) {
                Some(sid) => self.open_session(&params, Some(sid.to_string())).await,
                None => Err((INVALID_PARAMS, "sessionId is required".to_string())),
            },
            "session/prompt" => return self.prompt(id, params),
            _ => Err((METHOD_NOT_FOUND, format!("method not found: {method}"))),
        };
        match result {
            Ok(value) => self.out.respond(id, value),
            Err((code, message)) => self.out.respond_error(id, code, &message),
        }
    }

    fn slot(&self, params: &Value) -> Option<Arc<Slot>> {
        let sid = params.get("sessionId").and_then(Value::as_str)?;
        self.sessions.get(sid).cloned()
    }

    /// `session/new` (no `resume`) or `session/load`.
    async fn open_session(&mut self, params: &Value, resume: Option<String>) -> RpcResult {
        let Some(cwd) = params.get("cwd").and_then(Value::as_str) else {
            return Err((INVALID_PARAMS, "cwd is required".to_string()));
        };
        let cwd = PathBuf::from(cwd);
        let built = self
            .factory
            .build(&cwd, resume.as_deref())
            .await
            .map_err(|e| (INTERNAL_ERROR, format!("{e:#}")))?;
        let sid = built.session.id.clone();
        let open_tools = OpenTools::default();
        let prompter = AcpPrompter {
            out: self.out.clone(),
            session_id: sid.clone(),
            open_tools: Arc::clone(&open_tools),
            always: Mutex::new(HashSet::new()),
        };
        let agent = built
            .agent
            .with_permission_prompter(Arc::new(prompter))
            .with_question_prompter(Arc::new(AutoAnswerPrompter));
        if resume.is_some() {
            for update in map::history_updates(&built.session.messages) {
                self.out.notify(
                    "session/update",
                    json!({"sessionId": sid, "update": update}),
                );
            }
        }
        let slot = Slot {
            agent,
            session: tokio::sync::Mutex::new(built.session),
            route: built.route,
            cwd,
            cancel: Mutex::new(None),
            open_tools,
        };
        self.sessions.insert(sid.clone(), Arc::new(slot));
        Ok(match resume {
            Some(_) => Value::Null,
            None => json!({"sessionId": sid}),
        })
    }

    fn prompt(&mut self, id: Value, params: Value) {
        let Some(slot) = self.slot(&params) else {
            self.out
                .respond_error(id, INVALID_PARAMS, "unknown sessionId");
            return;
        };
        let blocks = params
            .get("prompt")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let text = map::prompt_text(&blocks);
        if text.trim().is_empty() {
            self.out.respond_error(id, INVALID_PARAMS, "empty prompt");
            return;
        }
        let cancel = {
            let mut busy = lock(&slot.cancel);
            if busy.is_some() {
                drop(busy);
                self.out
                    .respond_error(id, INVALID_PARAMS, "a prompt is already running");
                return;
            }
            let flag = new_cancel_flag();
            *busy = Some(Arc::clone(&flag));
            flag
        };
        let out = self.out.clone();
        self.tasks.spawn(async move {
            let result = run_prompt(&slot, &out, &text, &cancel).await;
            // Free the session before answering: the client may send the
            // next prompt as soon as it reads the response.
            *lock(&slot.cancel) = None;
            match result {
                Ok(stop) => out.respond(id, json!({"stopReason": stop})),
                Err(message) => out.respond_error(id, INTERNAL_ERROR, &message),
            }
        });
    }

    /// Input closed: deny pending approvals, cancel running turns, wait.
    async fn shutdown(&mut self) {
        self.out.close_pending();
        for slot in self.sessions.values() {
            if let Some(flag) = lock(&slot.cancel).as_ref() {
                request_cancel(flag);
            }
        }
        while self.tasks.join_next().await.is_some() {}
    }
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "agentCapabilities": {
            "loadSession": true,
            "promptCapabilities": {"image": false, "audio": false, "embeddedContext": true},
        },
        "authMethods": [],
        "agentInfo": {
            "name": "whycodes",
            "title": "WhyCodes",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

/// One prompt turn; the stop reason on success.
async fn run_prompt(
    slot: &Slot,
    out: &Outbox,
    text: &str,
    cancel: &CancelFlag,
) -> Result<&'static str, String> {
    let mut session = slot.session.lock().await;
    let sid = session.id.clone();
    let (tx, rx) = mpsc::unbounded_channel();
    session.add_user_message(&expand_user_input(text, &slot.cwd));
    let mut opts = TurnOpts::new(&slot.route.provider, &slot.route.model, &slot.route.api_key);
    opts.events = Some(tx);
    opts.cancel = Some(Arc::clone(cancel));
    // The turn owns the sender, so `forward` drains every update and ends
    // when the turn does — before the response goes out.
    let (result, ()) = tokio::join!(
        slot.agent.run_turn_with_events(&mut session, opts),
        forward(rx, out, &sid, &slot.cwd, &slot.open_tools),
    );
    save_session(&session);
    drop(session);
    if cancel.load(std::sync::atomic::Ordering::SeqCst) {
        return Ok("cancelled");
    }
    result.map(|_| "end_turn").map_err(|e| e.to_string())
}

fn save_session(session: &Session) {
    let saved = open_db().and_then(|db| Ok(session.save_to_db(&db)?));
    if let Err(e) = saved {
        tracing::warn!(error = %e, "acp: session save failed");
    }
}

/// Turn events → `session/update`, tracking open tool calls for approvals.
async fn forward(
    mut rx: mpsc::UnboundedReceiver<TurnEvent>,
    out: &Outbox,
    sid: &str,
    cwd: &Path,
    open_tools: &OpenTools,
) {
    while let Some(ev) = rx.recv().await {
        match &ev {
            TurnEvent::ToolStart { id, name, .. } => {
                lock(open_tools).push((name.clone(), id.clone()));
            }
            TurnEvent::ToolEnd { id, .. } => lock(open_tools).retain(|(_, open)| open != id),
            _ => {}
        }
        if let Some(update) = map::update_for(&ev, cwd) {
            out.notify(
                "session/update",
                json!({"sessionId": sid, "update": update}),
            );
        }
    }
}

/// Tool approvals as `session/request_permission`.
struct AcpPrompter {
    out: Outbox,
    session_id: String,
    open_tools: OpenTools,
    /// Tools the user chose "always allow" for in this session.
    always: Mutex<HashSet<String>>,
}

impl PermissionPrompter for AcpPrompter {
    fn ask<'a>(&'a self, tool_name: &'a str, detail: &'a str) -> PermissionAskFuture<'a> {
        Box::pin(self.decide(tool_name, detail))
    }
}

impl AcpPrompter {
    async fn decide(&self, tool_name: &str, detail: &str) -> bool {
        if lock(&self.always).contains(tool_name) {
            return true;
        }
        let tool_call_id = self.tool_call_id(tool_name).await;
        let params = json!({
            "sessionId": self.session_id,
            "toolCall": {
                "toolCallId": tool_call_id,
                "title": tool_name,
                "content": [{"type": "content", "content": {"type": "text", "text": detail}}],
            },
            "options": [
                {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                {"optionId": "always", "name": format!("Always allow {tool_name}"), "kind": "allow_always"},
                {"optionId": "reject", "name": "Reject", "kind": "reject_once"},
            ],
        });
        let reply = self.out.request("session/request_permission", params).await;
        let outcome = reply.as_ref().and_then(|r| r.get("outcome"));
        let selected = outcome
            .filter(|o| o.get("outcome").and_then(Value::as_str) == Some("selected"))
            .and_then(|o| o.get("optionId"))
            .and_then(Value::as_str);
        match selected {
            Some("allow") => true,
            Some("always") => {
                lock(&self.always).insert(tool_name.to_string());
                true
            }
            _ => false,
        }
    }

    /// The open `tool_call` this approval belongs to. Its `ToolStart` is
    /// emitted before the gate asks, but reaches the forwarder through a
    /// channel, so wait briefly; fall back to a fresh id.
    async fn tool_call_id(&self, tool_name: &str) -> String {
        let deadline = tokio::time::Instant::now() + TOOL_CALL_WAIT;
        loop {
            let found = lock(&self.open_tools)
                .iter()
                .rev()
                .find(|(name, _)| name == tool_name)
                .map(|(_, id)| id.clone());
            if let Some(id) = found {
                return id;
            }
            if tokio::time::Instant::now() >= deadline {
                return format!("permission-{tool_name}");
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

/// The CLI's agent setup, as `generate` does it, rooted at the editor's `cwd`.
struct CliFactory<'a> {
    cli: &'a Cli,
}

impl Factory for CliFactory<'_> {
    async fn build(&self, cwd: &Path, resume: Option<&str>) -> anyhow::Result<Built> {
        let cli = self.cli;
        let mut config = Config::load_layered(cwd)
            .or_else(|_| Config::load_or_create())
            .unwrap_or_default();
        if cli.no_memory {
            config.memory.enabled = false;
        }
        config.load_command_files(cwd);
        let provider = resolve_provider(cli, &config);
        let model = resolve_model(cli, &config);
        let api_key = match get_api_key(&provider, &config).await {
            Some(k) => k,
            None if !whycodes_llm::provider_requires_api_key(&provider, Some(&config)) => {
                String::new()
            }
            None => anyhow::bail!(missing_api_key_message_for(&provider, Some(&config))),
        };
        let mut info = agent_info_for(cli, &config);
        info.permission = config.effective_permission(&info.permission);
        let file_index = whycodes_index::WorkspaceIndex::start(
            whycodes_index::WorkspaceIndex::project_roots(cwd),
        );
        let mut agent = Agent::new(info)
            .with_config(&config)
            .with_file_index(file_index)
            .with_mcp(&config)
            .await;
        agent.set_route(&provider, &model);
        apply_sticky_credential(&agent, &provider, &config, &api_key);
        maybe_inject_test_llm(&mut agent, &provider);
        let session = match resume {
            Some(id) => Session::load_from_db(&open_db()?, id)?
                .ok_or_else(|| anyhow::anyhow!("no saved session {id}"))?,
            None => {
                let system_prompt = with_project_memory(
                    &Agent::with_agents_md(&agent.system_prompt_for_route(&provider, &model), cwd),
                    cwd,
                    &config,
                    None,
                );
                Session::new(cwd.to_path_buf(), system_prompt)
            }
        };
        Ok(Built {
            agent,
            session,
            route: Route {
                provider,
                model,
                api_key,
            },
        })
    }
}

#[cfg(test)]
#[path = "acp_tests.rs"]
mod tests;
