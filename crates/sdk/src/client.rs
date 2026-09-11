//! HTTP client for protocol v1.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use futures::StreamExt;
use tokio::process::{Child, Command};
use whycodes_protocol::sdk::{
    CompactRequest, CreateSessionRequest, ErrorCode, Handshake, HistoryMessage, ModelList,
    PROTOCOL_MAJOR, PermissionDecision, PermissionResponse, QuestionResponse, RenameRequest,
    RewindRequest, RunRequest, SdkEvent, SessionHistory, SessionInfo, SessionList, SetModelRequest,
    StructuredAttempt, StructuredResult, ToolCallSummary, TurnResult, UsageSnapshot, extract_json,
    validate_instance, validate_schema,
};

use crate::SdkError;

/// Options for [`WhyCodesClient::launch`].
#[derive(Debug, Clone)]
pub struct LaunchOptions {
    pub working_dir: PathBuf,
    /// Bind port. `None` picks an ephemeral loopback port.
    pub port: Option<u16>,
    /// `whycodes` binary. Falls back to `$WHYCODES`, then PATH.
    pub binary: Option<PathBuf>,
    pub startup_timeout: Duration,
    /// When true (default), the child uses the user's config/auth/home.
    /// When false, a private `WHYCODES_HOME` is used and API-key env vars
    /// are stripped so the instance cannot spend the user's quota.
    pub inherit_logins: bool,
    /// Explicit `WHYCODES_HOME`. Implies isolation even if `inherit_logins`.
    pub home: Option<PathBuf>,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            working_dir: working_dir_from(std::env::current_dir()),
            port: None,
            binary: None,
            startup_timeout: Duration::from_secs(15),
            inherit_logins: true,
            home: None,
        }
    }
}

fn working_dir_from(cwd: std::io::Result<PathBuf>) -> PathBuf {
    cwd.unwrap_or_else(|_| PathBuf::from("."))
}

/// Options for [`WhyCodesClient::run`].
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub max_turns: Option<usize>,
    /// When `None`, [`WhyCodesClient::run`] defaults to `true` (scripts) and
    /// [`WhyCodesClient::run_events`] defaults to `false` (interactive UIs).
    pub auto_approve: Option<bool>,
}

/// Connection to a `whycodes serve` process.
pub struct WhyCodesClient {
    base: String,
    http: reqwest::Client,
    child: Option<Child>,
    /// Kept alive so an isolated temp home is not deleted while the daemon runs.
    _home: Option<tempfile::TempDir>,
}

impl WhyCodesClient {
    /// Attach to an already-running daemon (`whycodes serve`).
    pub async fn connect(base: &str) -> Result<Self, SdkError> {
        let base = normalize_base(base);
        let http = http_client()?;
        let client = Self {
            base,
            http,
            child: None,
            _home: None,
        };
        client.handshake().await?;
        Ok(client)
    }

    #[cfg(test)]
    pub(crate) fn unconnected(base: &str, http: reqwest::Client) -> Self {
        Self {
            base: normalize_base(base),
            http,
            child: None,
            _home: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_child(mut self, child: Child) -> Self {
        self.child = Some(child);
        self
    }

    /// Spawn `whycodes serve` as a private instance and connect to it.
    ///
    /// The child inherits this process's environment (API keys, `HOME`), so
    /// it spends the same provider quota as the user. `close` / drop kills it.
    pub async fn launch(opts: LaunchOptions) -> Result<Self, SdkError> {
        let prepared = prepare_launch(&opts)?;
        if let Some(err) = missing_spawned_binary(&prepared.binary) {
            return Err(err);
        }
        let mut cmd = launch_command(&prepared, &opts);
        let child = cmd.spawn().map_err(|e| {
            SdkError::with_source(
                ErrorCode::ServeNotFound,
                &format!("could not execute {}: {e}", prepared.binary.display()),
                e,
            )
        })?;
        let port = prepared.port;
        let held_home = prepared.held_home;

        let base = format!("http://127.0.0.1:{port}");
        let http = http_client()?;
        let mut client = Self {
            base: base.clone(),
            http,
            child: Some(child),
            _home: held_home,
        };

        let deadline = Instant::now() + opts.startup_timeout;
        loop {
            let child_status = poll_child_exit(client.child.as_mut());
            let handshake = client.handshake().await.map(|_| ());
            match launch_poll(
                Instant::now(),
                deadline,
                opts.startup_timeout,
                &base,
                child_status,
                handshake,
            ) {
                LaunchPoll::Ready => return Ok(client),
                LaunchPoll::Retry => tokio::time::sleep(Duration::from_millis(50)).await,
                LaunchPoll::Failed(err) => {
                    let stderr = take_stderr(&mut client.child).await;
                    return Err(attach_stderr(err, &stderr));
                }
            }
        }
    }

    /// Daemon base URL (`http://127.0.0.1:3030`).
    pub fn base_url(&self) -> &str {
        &self.base
    }

    pub async fn health(&self) -> Result<Handshake, SdkError> {
        self.handshake().await
    }

    pub async fn list_sessions(&self) -> Result<Vec<SessionInfo>, SdkError> {
        let url = format!("{}/v1/sessions", self.base);
        let res = self.http.get(&url).send().await?;
        if !res.status().is_success() {
            return Err(status_error(res.status(), "list sessions"));
        }
        let body: SessionList = res.json().await?;
        Ok(body.sessions)
    }

    pub async fn create_session(&self, project: Option<&str>) -> Result<SessionInfo, SdkError> {
        let url = format!("{}/v1/sessions", self.base);
        let req = CreateSessionRequest {
            project: project.map(str::to_string),
            persist: Some(true),
        };
        let res = self.http.post(&url).json(&req).send().await?;
        if !res.status().is_success() {
            return Err(status_error(res.status(), "create session"));
        }
        Ok(res.json().await?)
    }

    pub async fn get_session(&self, id: &str) -> Result<SessionInfo, SdkError> {
        let url = format!("{}/v1/sessions/{id}", self.base);
        let res = self.http.get(&url).send().await?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SdkError::new(
                ErrorCode::UnknownSession,
                &format!("session {id} not found"),
            ));
        }
        if !res.status().is_success() {
            return Err(status_error(res.status(), "get session"));
        }
        Ok(res.json().await?)
    }

    /// Full transcript (`limit` keeps the last N messages).
    pub async fn get_history(
        &self,
        session_id: &str,
        limit: Option<usize>,
    ) -> Result<SessionHistory, SdkError> {
        let mut url = format!("{}/v1/sessions/{session_id}/messages", self.base);
        if let Some(n) = limit {
            url.push_str(&format!("?limit={n}"));
        }
        let res = self.http.get(&url).send().await?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SdkError::new(
                ErrorCode::UnknownSession,
                &format!("session {session_id} not found"),
            ));
        }
        if !res.status().is_success() {
            return Err(status_error(res.status(), "history"));
        }
        Ok(res.json().await?)
    }

    pub async fn peek(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<HistoryMessage>, SdkError> {
        Ok(self.get_history(session_id, Some(limit)).await?.messages)
    }

    pub async fn list_models(&self) -> Result<ModelList, SdkError> {
        let url = format!("{}/v1/models", self.base);
        let res = self.http.get(&url).send().await?;
        if !res.status().is_success() {
            return Err(status_error(res.status(), "list models"));
        }
        Ok(res.json().await?)
    }

    pub async fn set_model(
        &self,
        session_id: &str,
        provider: &str,
        model: &str,
    ) -> Result<(), SdkError> {
        let url = format!("{}/v1/sessions/{session_id}/model", self.base);
        let req = SetModelRequest {
            provider: provider.to_string(),
            model: model.to_string(),
        };
        let res = self.http.post(&url).json(&req).send().await?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SdkError::new(
                ErrorCode::UnknownSession,
                &format!("session {session_id} not found"),
            ));
        }
        if !res.status().is_success() {
            return Err(status_error(res.status(), "set model"));
        }
        Ok(())
    }

    pub async fn rename_session(
        &self,
        session_id: &str,
        title: &str,
    ) -> Result<SessionInfo, SdkError> {
        let url = format!("{}/v1/sessions/{session_id}/rename", self.base);
        let req = RenameRequest {
            title: title.to_string(),
        };
        let res = self.http.post(&url).json(&req).send().await?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SdkError::new(
                ErrorCode::UnknownSession,
                &format!("session {session_id} not found"),
            ));
        }
        if !res.status().is_success() {
            return Err(status_error(res.status(), "rename"));
        }
        Ok(res.json().await?)
    }

    pub async fn rewind(&self, session_id: &str, index: usize) -> Result<SessionHistory, SdkError> {
        let url = format!("{}/v1/sessions/{session_id}/rewind", self.base);
        let req = RewindRequest { index };
        let res = self.http.post(&url).json(&req).send().await?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SdkError::new(
                ErrorCode::UnknownSession,
                &format!("session {session_id} not found"),
            ));
        }
        if !res.status().is_success() {
            return Err(status_error(res.status(), "rewind"));
        }
        Ok(res.json().await?)
    }

    pub async fn compact(
        &self,
        session_id: &str,
        max_tokens: Option<usize>,
    ) -> Result<SessionHistory, SdkError> {
        let url = format!("{}/v1/sessions/{session_id}/compact", self.base);
        let req = CompactRequest { max_tokens };
        let res = self.http.post(&url).json(&req).send().await?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SdkError::new(
                ErrorCode::UnknownSession,
                &format!("session {session_id} not found"),
            ));
        }
        if !res.status().is_success() {
            return Err(status_error(res.status(), "compact"));
        }
        Ok(res.json().await?)
    }

    pub async fn respond_to_question(
        &self,
        session_id: &str,
        request_id: &str,
        answers: Option<Vec<whycodes_protocol::sdk::QuestionAnswerWire>>,
        cancelled: bool,
    ) -> Result<(), SdkError> {
        let url = format!("{}/v1/sessions/{session_id}/question", self.base);
        let req = QuestionResponse {
            request_id: request_id.to_string(),
            answers,
            cancelled: Some(cancelled),
        };
        let res = self.http.post(&url).json(&req).send().await?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SdkError::new(
                ErrorCode::UnknownSession,
                "unknown question request",
            ));
        }
        if !res.status().is_success() {
            return Err(status_error(res.status(), "question"));
        }
        Ok(())
    }

    /// Run one turn and collect the result. Use [`Self::run_events`] for a live UI.
    ///
    /// Defaults `auto_approve` to true so scripts do not hang on `Ask`.
    pub async fn run(
        &self,
        session_id: &str,
        message: &str,
        mut opts: RunOptions,
    ) -> Result<TurnResult, SdkError> {
        if opts.auto_approve.is_none() {
            opts.auto_approve = Some(true);
        }
        let mut text = String::new();
        let mut tool_calls = Vec::new();
        let mut tool_names: Vec<(String, String)> = Vec::new();
        let mut usage = None;
        let mut cancelled = false;
        let mut last_error: Option<SdkError> = None;

        let mut stream = self.run_events(session_id, message, opts).await?;
        while let Some(item) = stream.next().await {
            match item? {
                SdkEvent::TextDelta { text: delta } => text.push_str(&delta),
                SdkEvent::ToolStart { id, name, .. } => {
                    tool_names.push((id, name));
                }
                SdkEvent::ToolEnd { id, is_error, .. } => {
                    let name = tool_names
                        .iter()
                        .find(|(tid, _)| tid == &id)
                        .map(|(_, n)| n.clone())
                        .unwrap_or_default();
                    tool_calls.push(ToolCallSummary { id, name, is_error });
                }
                SdkEvent::Usage {
                    input_tokens,
                    output_tokens,
                    cache_read_input_tokens,
                    cache_creation_input_tokens,
                } => {
                    usage = Some(UsageSnapshot {
                        input_tokens,
                        output_tokens,
                        cache_read_input_tokens,
                        cache_creation_input_tokens,
                    });
                }
                SdkEvent::Cancelled => cancelled = true,
                SdkEvent::TurnDone { text: done } => {
                    if text.is_empty() && !done.is_empty() {
                        text = done;
                    }
                }
                SdkEvent::Error { code, message } => {
                    last_error = Some(SdkError::new(code, &message));
                }
                _ => {}
            }
        }

        if let Some(err) = last_error {
            return Err(err);
        }
        Ok(TurnResult {
            text,
            tool_calls,
            usage,
            cancelled,
        })
    }

    /// Stream protocol events for one turn (SSE).
    pub async fn run_events(
        &self,
        session_id: &str,
        message: &str,
        opts: RunOptions,
    ) -> Result<EventStream, SdkError> {
        let url = format!("{}/v1/sessions/{session_id}/run", self.base);
        let req = RunRequest {
            message: message.to_string(),
            provider: opts.provider,
            model: opts.model,
            max_turns: opts.max_turns,
            auto_approve: opts.auto_approve,
        };
        let res = self.http.post(&url).json(&req).send().await?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SdkError::new(
                ErrorCode::UnknownSession,
                &format!("session {session_id} not found"),
            ));
        }
        if res.status() == reqwest::StatusCode::BAD_REQUEST {
            return Err(SdkError::new(ErrorCode::InvalidRequest, "empty message"));
        }
        if !res.status().is_success() {
            return Err(status_error(res.status(), "run"));
        }
        Ok(EventStream {
            bytes: res.bytes_stream().map(|r| r.map(|b| b.to_vec())).boxed(),
            buf: String::new(),
            done: false,
        })
    }

    /// Answer a [`SdkEvent::PermissionRequest`].
    pub async fn respond_to_permission(
        &self,
        session_id: &str,
        request_id: &str,
        decision: PermissionDecision,
    ) -> Result<(), SdkError> {
        let url = format!("{}/v1/sessions/{session_id}/permission", self.base);
        let req = PermissionResponse {
            request_id: request_id.to_string(),
            decision,
        };
        let res = self.http.post(&url).json(&req).send().await?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SdkError::new(
                ErrorCode::UnknownSession,
                "unknown permission request",
            ));
        }
        if !res.status().is_success() {
            return Err(status_error(res.status(), "permission"));
        }
        Ok(())
    }

    /// Run turns until the model returns JSON that matches `schema`.
    ///
    /// `max_retries` extra corrective turns after the first (default 2).
    pub async fn run_structured(
        &self,
        session_id: &str,
        message: &str,
        schema: serde_json::Value,
        opts: RunOptions,
        max_retries: Option<u32>,
    ) -> Result<StructuredResult, SdkError> {
        if let Err(e) = validate_schema(&schema) {
            return Err(SdkError::new(ErrorCode::StructuredSchemaInvalid, &e));
        }
        let retries = max_retries.unwrap_or(2);
        let schema_txt = schema_text(&schema);
        let mut prompt = format!(
            "{message}\n\nReply with a single JSON value that validates against this schema. \
             No markdown, no commentary.\n{schema_txt}"
        );
        let mut attempts = Vec::new();
        let mut i = 0;
        loop {
            let turn = self.run(session_id, &prompt, opts.clone()).await?;
            match extract_json(&turn.text) {
                Ok(data) => {
                    let errors = validate_instance(&schema, &data);
                    let ok = errors.is_empty();
                    attempts.push(StructuredAttempt {
                        text: turn.text.clone(),
                        ok,
                        errors: errors.clone(),
                    });
                    if ok {
                        return Ok(StructuredResult { data, attempts });
                    }
                    if i == retries {
                        return Err(SdkError::new(
                            ErrorCode::StructuredOutputInvalid,
                            &errors.join("; "),
                        ));
                    }
                    prompt = format!(
                        "Your previous reply was not valid JSON for the schema.\nErrors:\n- {}\n\
                         Reply again with only the JSON value.",
                        errors.join("\n- ")
                    );
                }
                Err(e) => {
                    attempts.push(StructuredAttempt {
                        text: turn.text.clone(),
                        ok: false,
                        errors: vec![e.clone()],
                    });
                    if i == retries {
                        return Err(SdkError::new(ErrorCode::StructuredOutputInvalid, &e));
                    }
                    prompt = format!(
                        "Your previous reply was not parseable JSON ({e}). \
                         Reply again with only the JSON value matching the schema."
                    );
                }
            }
            i += 1;
        }
    }

    /// Ask the daemon to cancel an in-flight run.
    pub async fn cancel(&self, session_id: &str) -> Result<(), SdkError> {
        let url = format!("{}/v1/sessions/{session_id}/cancel", self.base);
        let res = self.http.post(&url).send().await?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SdkError::new(
                ErrorCode::UnknownSession,
                &format!("no in-flight run for {session_id}"),
            ));
        }
        if !res.status().is_success() {
            return Err(status_error(res.status(), "cancel"));
        }
        Ok(())
    }

    /// Stop a launched child. No-op for [`Self::connect`].
    pub async fn close(mut self) -> Result<(), SdkError> {
        self.kill_child().await;
        Ok(())
    }

    async fn handshake(&self) -> Result<Handshake, SdkError> {
        let url = format!("{}/v1/health", self.base);
        let res = self.http.get(&url).send().await.map_err(|e| {
            if e.is_connect() {
                SdkError::with_source(
                    ErrorCode::Disconnected,
                    &format!("cannot reach {}: {e}", self.base),
                    e,
                )
            } else {
                e.into()
            }
        })?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SdkError::new(
                ErrorCode::UnsupportedVersion,
                &format!(
                    "{} has no /v1/health — upgrade whycodes serve (need protocol {PROTOCOL_MAJOR})",
                    self.base
                ),
            ));
        }
        if !res.status().is_success() {
            return Err(status_error(res.status(), "health"));
        }
        let hs: Handshake = res.json().await?;
        if hs.protocol != PROTOCOL_MAJOR {
            return Err(SdkError::new(
                ErrorCode::UnsupportedVersion,
                &format!(
                    "daemon speaks protocol {}, client speaks {PROTOCOL_MAJOR}",
                    hs.protocol
                ),
            ));
        }
        Ok(hs)
    }

    async fn kill_child(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _kill = child.kill().await;
            let _wait = child.wait().await;
        }
    }
}

impl Drop for WhyCodesClient {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _kill = child.start_kill();
        }
    }
}

/// Stream of [`SdkEvent`] from one `/v1/sessions/:id/run`.
pub struct EventStream {
    bytes: futures::stream::BoxStream<'static, reqwest::Result<Vec<u8>>>,
    buf: String,
    done: bool,
}

impl futures::Stream for EventStream {
    type Item = Result<SdkEvent, SdkError>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use std::task::Poll;
        if self.done {
            return Poll::Ready(None);
        }
        loop {
            if let Some(data) = pop_sse_data(&mut self.buf) {
                match serde_json::from_str::<SdkEvent>(&data) {
                    Ok(SdkEvent::TurnDone { text }) => {
                        self.done = true;
                        return Poll::Ready(Some(Ok(SdkEvent::TurnDone { text })));
                    }
                    Ok(ev) => return Poll::Ready(Some(Ok(ev))),
                    Err(e) => {
                        return Poll::Ready(Some(Err(SdkError::with_source(
                            ErrorCode::Internal,
                            &format!("bad event: {e}"),
                            e,
                        ))));
                    }
                }
            }
            match self.bytes.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    self.buf
                        .push_str(&String::from_utf8_lossy(&chunk).replace("\r\n", "\n"));
                }
                Poll::Ready(Some(Err(e))) => {
                    self.done = true;
                    return Poll::Ready(Some(Err(e.into())));
                }
                Poll::Ready(None) => {
                    self.done = true;
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

fn pop_sse_data(buf: &mut String) -> Option<String> {
    let idx = buf.find("\n\n")?;
    let frame = buf[..idx].to_string();
    *buf = buf[idx + 2..].to_string();
    let mut data = String::new();
    for line in frame.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.trim_start());
        }
    }
    if data.is_empty() { None } else { Some(data) }
}

fn http_client_failed(e: reqwest::Error) -> SdkError {
    SdkError::with_source(ErrorCode::Internal, "http client", e)
}

fn http_client() -> Result<reqwest::Client, SdkError> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(http_client_failed)
}

fn schema_text(schema: &serde_json::Value) -> String {
    schema_text_from(serde_json::to_string_pretty(schema), schema)
}

fn schema_text_from(
    pretty: std::result::Result<String, serde_json::Error>,
    schema: &serde_json::Value,
) -> String {
    pretty.unwrap_or_else(|_| schema.to_string())
}

pub(crate) fn normalize_base(addr: &str) -> String {
    let t = addr.trim().trim_end_matches('/');
    if t.starts_with("http://") || t.starts_with("https://") {
        t.to_string()
    } else {
        format!("http://{t}")
    }
}

fn process_exe() -> std::io::Result<PathBuf> {
    #[cfg(test)]
    {
        if std::env::var_os("WHYCODES_TEST_CURRENT_EXE_FAIL").is_some() {
            return Err(std::io::Error::other("injected current_exe failure"));
        }
        if let Ok(p) = std::env::var("WHYCODES_TEST_CURRENT_EXE")
            && !p.is_empty()
        {
            return Ok(PathBuf::from(p));
        }
    }
    std::env::current_exe()
}

fn resolve_binary(explicit: Option<&Path>) -> Result<PathBuf, SdkError> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    if let Ok(p) = std::env::var("WHYCODES")
        && !p.is_empty()
    {
        return Ok(PathBuf::from(p));
    }
    if let Ok(exe) = process_exe() {
        if exe
            .file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|n| n == "whycodes")
        {
            return Ok(exe);
        }
        for name in binary_names() {
            let sibling = exe.with_file_name(name);
            if sibling.is_file() {
                return Ok(sibling);
            }
        }
    }
    Ok(PathBuf::from("whycodes"))
}

fn binary_names() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        &["whycodes.exe"]
    }
    #[cfg(not(windows))]
    {
        &["whycodes"]
    }
}

#[cfg(test)]
pub(crate) fn ephemeral_port_for_test() -> u16 {
    ephemeral_port().expect("ephemeral port")
}

fn ephemeral_bind_failed(e: std::io::Error) -> SdkError {
    SdkError::with_source(ErrorCode::StartupFailed, "bind ephemeral port", e)
}

fn ephemeral_addr_failed(e: std::io::Error) -> SdkError {
    SdkError::with_source(ErrorCode::StartupFailed, "ephemeral port", e)
}

fn ephemeral_port() -> Result<u16, SdkError> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(ephemeral_bind_failed)?;
    listener
        .local_addr()
        .map(|a| a.port())
        .map_err(ephemeral_addr_failed)
}

const STRIPPED_LOGIN_KEYS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "XAI_API_KEY",
    "GROQ_API_KEY",
    "GOOGLE_API_KEY",
    "DEEPSEEK_API_KEY",
    "MISTRAL_API_KEY",
];

struct PreparedLaunch {
    port: u16,
    binary: PathBuf,
    home_env: Option<PathBuf>,
    held_home: Option<tempfile::TempDir>,
}

/// Missing binary is ServeNotFound even when spawn returns Ok (some Linux
/// hosts delay the 127 until wait). Do not wait for try_wait — handshake
/// timeout would otherwise map to StartupFailed.
///
/// Bare names (`whycodes`, `python`) go through PATH and are not files.
fn missing_spawned_binary(binary: &Path) -> Option<SdkError> {
    if !looks_like_filesystem_path(binary) || binary.is_file() {
        None
    } else {
        Some(SdkError::new(
            ErrorCode::ServeNotFound,
            &format!("could not execute {}: not found", binary.display()),
        ))
    }
}

fn looks_like_filesystem_path(binary: &Path) -> bool {
    binary.is_absolute() || binary.parent().is_some_and(|p| !p.as_os_str().is_empty())
}

fn poll_child_exit(child: Option<&mut Child>) -> Option<String> {
    let child = child?;
    match child.try_wait() {
        Ok(Some(status)) => Some(status.to_string()),
        _ => None,
    }
}

fn create_temp_home() -> std::io::Result<tempfile::TempDir> {
    #[cfg(test)]
    if std::env::var_os("WHYCODES_TEST_TEMPDIR_FAIL").is_some() {
        return Err(std::io::Error::other("injected tempdir failure"));
    }
    tempfile::tempdir()
}

fn prepare_launch(opts: &LaunchOptions) -> Result<PreparedLaunch, SdkError> {
    let port = match opts.port {
        Some(p) => p,
        None => ephemeral_port()?,
    };
    let binary = resolve_binary(opts.binary.as_deref())?;
    let isolated = !opts.inherit_logins || opts.home.is_some();
    let (home_env, held_home) = if isolated {
        if let Some(p) = opts.home.clone() {
            std::fs::create_dir_all(&p).map_err(|e| {
                SdkError::with_source(ErrorCode::StartupFailed, "create WHYCODES_HOME", e)
            })?;
            (Some(p), None)
        } else {
            let tmp = create_temp_home().map_err(|e| {
                SdkError::with_source(ErrorCode::StartupFailed, "temp WHYCODES_HOME", e)
            })?;
            let path = tmp.path().to_path_buf();
            (Some(path), Some(tmp))
        }
    } else {
        (None, None)
    };
    Ok(PreparedLaunch {
        port,
        binary,
        home_env,
        held_home,
    })
}

fn launch_command(prepared: &PreparedLaunch, opts: &LaunchOptions) -> Command {
    let mut cmd = Command::new(&prepared.binary);
    cmd.arg("serve")
        .arg(prepared.port.to_string())
        .current_dir(&opts.working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(home) = &prepared.home_env {
        cmd.env("WHYCODES_HOME", home);
    }
    if !opts.inherit_logins {
        for key in STRIPPED_LOGIN_KEYS {
            cmd.env_remove(key);
        }
    }
    cmd
}

#[derive(Debug)]
enum LaunchPoll {
    Ready,
    Retry,
    Failed(SdkError),
}

fn launch_poll(
    now: Instant,
    deadline: Instant,
    timeout: Duration,
    base: &str,
    child_exited: Option<String>,
    handshake: Result<(), SdkError>,
) -> LaunchPoll {
    if now >= deadline {
        return LaunchPoll::Failed(SdkError::new(
            ErrorCode::StartupTimeout,
            &format!("daemon at {base} did not become healthy in {timeout:?}."),
        ));
    }
    if let Some(status) = child_exited {
        return LaunchPoll::Failed(SdkError::new(
            ErrorCode::StartupFailed,
            &format!("whycodes serve exited ({status})."),
        ));
    }
    match handshake {
        Ok(()) => LaunchPoll::Ready,
        Err(e) if matches!(e.code, ErrorCode::UnsupportedVersion) => LaunchPoll::Failed(e),
        Err(_retry) => LaunchPoll::Retry,
    }
}

fn attach_stderr(err: SdkError, stderr: &str) -> SdkError {
    if stderr.is_empty() {
        err
    } else {
        SdkError::new(err.code, &format!("{} {stderr}", err.message))
    }
}

fn status_error(status: reqwest::StatusCode, what: &str) -> SdkError {
    let code = if status == reqwest::StatusCode::NOT_FOUND {
        ErrorCode::UnknownSession
    } else if status == reqwest::StatusCode::BAD_REQUEST {
        ErrorCode::InvalidRequest
    } else if status == reqwest::StatusCode::UNAUTHORIZED {
        ErrorCode::Auth
    } else {
        ErrorCode::Internal
    };
    SdkError::new(code, &format!("{what} failed: {status}"))
}

async fn take_stderr(child: &mut Option<Child>) -> String {
    let Some(child) = child.as_mut() else {
        return String::new();
    };
    let Some(mut stderr) = child.stderr.take() else {
        return String::new();
    };
    // A live child holds the pipe open; kill it so `read_to_end` can finish.
    let _kill = child.start_kill();
    let mut buf = Vec::new();
    let _read = tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut buf).await;
    let s = String::from_utf8_lossy(&buf);
    let trimmed = s.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("stderr: {trimmed}")
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
