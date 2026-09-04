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
            working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            port: None,
            binary: None,
            startup_timeout: Duration::from_secs(15),
            inherit_logins: true,
            home: None,
        }
    }
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
    pub async fn connect(base: impl AsRef<str>) -> Result<Self, SdkError> {
        let base = normalize_base(base.as_ref());
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
    pub(crate) fn unconnected(base: impl AsRef<str>, http: reqwest::Client) -> Self {
        Self {
            base: normalize_base(base.as_ref()),
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
                let tmp = tempfile::tempdir().map_err(|e| {
                    SdkError::with_source(ErrorCode::StartupFailed, "temp WHYCODES_HOME", e)
                })?;
                let path = tmp.path().to_path_buf();
                (Some(path), Some(tmp))
            }
        } else {
            (None, None)
        };
        let mut cmd = Command::new(&binary);
        cmd.arg("serve")
            .arg(port.to_string())
            .current_dir(&opts.working_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(home) = &home_env {
            cmd.env("WHYCODES_HOME", home);
        }
        if !opts.inherit_logins {
            for key in [
                "ANTHROPIC_API_KEY",
                "OPENAI_API_KEY",
                "OPENROUTER_API_KEY",
                "XAI_API_KEY",
                "GROQ_API_KEY",
                "GOOGLE_API_KEY",
                "DEEPSEEK_API_KEY",
                "MISTRAL_API_KEY",
            ] {
                cmd.env_remove(key);
            }
        }
        let child = cmd.spawn().map_err(|e| {
            SdkError::with_source(
                ErrorCode::ServeNotFound,
                format!("could not execute {}: {e}", binary.display()),
                e,
            )
        })?;

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
            if Instant::now() >= deadline {
                let stderr = take_stderr(&mut client.child).await;
                return Err(SdkError::new(
                    ErrorCode::StartupTimeout,
                    format!(
                        "daemon at {base} did not become healthy in {:?}. {stderr}",
                        opts.startup_timeout
                    ),
                ));
            }
            if let Some(child) = client.child.as_mut()
                && let Ok(Some(status)) = child.try_wait()
            {
                let stderr = take_stderr(&mut client.child).await;
                return Err(SdkError::new(
                    ErrorCode::StartupFailed,
                    format!("whycodes serve exited ({status}). {stderr}"),
                ));
            }
            match client.handshake().await {
                Ok(_) => return Ok(client),
                Err(e) if matches!(e.code, ErrorCode::UnsupportedVersion) => return Err(e),
                Err(_retry) => tokio::time::sleep(Duration::from_millis(50)).await,
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

    pub async fn create_session(
        &self,
        project: Option<impl Into<String>>,
    ) -> Result<SessionInfo, SdkError> {
        let url = format!("{}/v1/sessions", self.base);
        let req = CreateSessionRequest {
            project: project.map(Into::into),
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
                format!("session {id} not found"),
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
                format!("session {session_id} not found"),
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
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<(), SdkError> {
        let url = format!("{}/v1/sessions/{session_id}/model", self.base);
        let req = SetModelRequest {
            provider: provider.into(),
            model: model.into(),
        };
        let res = self.http.post(&url).json(&req).send().await?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SdkError::new(
                ErrorCode::UnknownSession,
                format!("session {session_id} not found"),
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
        title: impl Into<String>,
    ) -> Result<SessionInfo, SdkError> {
        let url = format!("{}/v1/sessions/{session_id}/rename", self.base);
        let req = RenameRequest {
            title: title.into(),
        };
        let res = self.http.post(&url).json(&req).send().await?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SdkError::new(
                ErrorCode::UnknownSession,
                format!("session {session_id} not found"),
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
                format!("session {session_id} not found"),
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
                format!("session {session_id} not found"),
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
        request_id: impl Into<String>,
        answers: Option<Vec<whycodes_protocol::sdk::QuestionAnswerWire>>,
        cancelled: bool,
    ) -> Result<(), SdkError> {
        let url = format!("{}/v1/sessions/{session_id}/question", self.base);
        let req = QuestionResponse {
            request_id: request_id.into(),
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
        message: impl Into<String>,
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
                    last_error = Some(SdkError::new(code, message));
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
        message: impl Into<String>,
        opts: RunOptions,
    ) -> Result<EventStream, SdkError> {
        let url = format!("{}/v1/sessions/{session_id}/run", self.base);
        let req = RunRequest {
            message: message.into(),
            provider: opts.provider,
            model: opts.model,
            max_turns: opts.max_turns,
            auto_approve: opts.auto_approve,
        };
        let res = self.http.post(&url).json(&req).send().await?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SdkError::new(
                ErrorCode::UnknownSession,
                format!("session {session_id} not found"),
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
        request_id: impl Into<String>,
        decision: PermissionDecision,
    ) -> Result<(), SdkError> {
        let url = format!("{}/v1/sessions/{session_id}/permission", self.base);
        let req = PermissionResponse {
            request_id: request_id.into(),
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
        message: impl Into<String>,
        schema: serde_json::Value,
        opts: RunOptions,
        max_retries: Option<u32>,
    ) -> Result<StructuredResult, SdkError> {
        if let Err(e) = validate_schema(&schema) {
            return Err(SdkError::new(ErrorCode::StructuredSchemaInvalid, e));
        }
        let retries = max_retries.unwrap_or(2);
        let schema_txt =
            serde_json::to_string_pretty(&schema).unwrap_or_else(|_| schema.to_string());
        let mut prompt = format!(
            "{}\n\nReply with a single JSON value that validates against this schema. \
             No markdown, no commentary.\n{schema_txt}",
            message.into()
        );
        let mut attempts = Vec::new();
        let mut i = 0;
        loop {
            let turn = self.run(session_id, prompt.clone(), opts.clone()).await?;
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
                            errors.join("; "),
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
                        return Err(SdkError::new(ErrorCode::StructuredOutputInvalid, e));
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
                format!("no in-flight run for {session_id}"),
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
                    format!("cannot reach {}: {e}", self.base),
                    e,
                )
            } else {
                e.into()
            }
        })?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SdkError::new(
                ErrorCode::UnsupportedVersion,
                format!(
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
                format!(
                    "daemon speaks protocol {}, client speaks {PROTOCOL_MAJOR}",
                    hs.protocol
                ),
            ));
        }
        Ok(hs)
    }

    async fn kill_child(&mut self) {
        if let Some(mut child) = self.child.take() {
            if let Err(_kill) = child.kill().await {
                // Best-effort teardown of a launched daemon.
            }
            if let Err(_wait) = child.wait().await {
                // Process already gone.
            }
        }
    }
}

impl Drop for WhyCodesClient {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take()
            && let Err(_kill) = child.start_kill()
        {
            // Drop cannot await a graceful wait.
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
                            format!("bad event: {e}"),
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

fn http_client() -> Result<reqwest::Client, SdkError> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| SdkError::with_source(ErrorCode::Internal, "http client", e))
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

fn ephemeral_port() -> Result<u16, SdkError> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| SdkError::with_source(ErrorCode::StartupFailed, "bind ephemeral port", e))?;
    listener
        .local_addr()
        .map(|a| a.port())
        .map_err(|e| SdkError::with_source(ErrorCode::StartupFailed, "ephemeral port", e))
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
    SdkError::new(code, format!("{what} failed: {status}"))
}

async fn take_stderr(child: &mut Option<Child>) -> String {
    let Some(child) = child.as_mut() else {
        return String::new();
    };
    let Some(mut stderr) = child.stderr.take() else {
        return String::new();
    };
    let mut buf = Vec::new();
    if let Err(_read) = tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut buf).await {
        // Diagnostic only; empty stderr is fine.
    }
    let s = String::from_utf8_lossy(&buf);
    let trimmed = s.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("stderr: {trimmed}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn normalize_adds_scheme() {
        assert_eq!(normalize_base("127.0.0.1:3030"), "http://127.0.0.1:3030");
        assert_eq!(
            normalize_base("http://localhost:3030/"),
            "http://localhost:3030"
        );
    }

    #[test]
    fn pop_sse_skips_keepalive() {
        let mut buf = ": ping\n\ndata: {\"ev\":\"cancelled\"}\n\n".to_string();
        assert!(pop_sse_data(&mut buf).is_none());
        let data = pop_sse_data(&mut buf).unwrap();
        assert!(data.contains("cancelled"));
    }

    #[test]
    fn pop_sse_joins_multiline_data() {
        let mut buf = "data: one\ndata: two\n\n".to_string();
        assert_eq!(pop_sse_data(&mut buf).as_deref(), Some("one\ntwo"));
        assert!(pop_sse_data(&mut buf).is_none());
    }

    #[test]
    fn normalize_https_and_whitespace() {
        assert_eq!(
            normalize_base("  https://example.test/v1/  "),
            "https://example.test/v1"
        );
        assert_eq!(normalize_base("localhost:9"), "http://localhost:9");
    }

    #[test]
    fn resolve_binary_prefers_explicit_then_env() {
        let _lock = env_lock();
        let p = resolve_binary(Some(Path::new("/opt/whycodes"))).unwrap();
        assert_eq!(p, PathBuf::from("/opt/whycodes"));
        let prev = std::env::var_os("WHYCODES");
        unsafe { std::env::set_var("WHYCODES", "/env/whycodes") };
        let p = resolve_binary(None).unwrap();
        assert_eq!(p, PathBuf::from("/env/whycodes"));
        unsafe { std::env::set_var("WHYCODES", "") };
        let p = resolve_binary(None).unwrap();
        assert_eq!(p, PathBuf::from("whycodes"));
        match prev {
            Some(v) => unsafe { std::env::set_var("WHYCODES", v) },
            None => unsafe { std::env::remove_var("WHYCODES") },
        }
    }

    #[test]
    fn status_error_maps_http_codes() {
        assert_eq!(
            status_error(reqwest::StatusCode::NOT_FOUND, "x").code,
            ErrorCode::UnknownSession
        );
        assert_eq!(
            status_error(reqwest::StatusCode::BAD_REQUEST, "x").code,
            ErrorCode::InvalidRequest
        );
        assert_eq!(
            status_error(reqwest::StatusCode::UNAUTHORIZED, "x").code,
            ErrorCode::Auth
        );
        assert_eq!(
            status_error(reqwest::StatusCode::INTERNAL_SERVER_ERROR, "x").code,
            ErrorCode::Internal
        );
        let e = SdkError::new(ErrorCode::Timeout, "slow");
        assert!(e.to_string().contains("timeout") || e.to_string().contains("slow"));
    }

    #[test]
    fn ephemeral_port_binds() {
        let p = ephemeral_port().unwrap();
        assert!(p > 0);
    }

    #[test]
    fn launch_options_default() {
        let o = LaunchOptions::default();
        assert!(o.inherit_logins);
        assert!(o.binary.is_none());
        assert!(o.port.is_none());
        assert!(o.home.is_none());
        assert!(!o.working_dir.as_os_str().is_empty());
    }

    #[test]
    fn http_client_builds() {
        assert!(http_client().is_ok());
        assert_eq!(binary_names()[0], "whycodes");
    }

    #[cfg(windows)]
    #[test]
    fn binary_names_windows() {
        assert_eq!(binary_names(), &["whycodes.exe"]);
    }

    #[test]
    fn resolve_binary_uses_injected_exe_and_sibling() {
        let _lock = env_lock();
        let prev_why = std::env::var_os("WHYCODES");
        let prev_exe = std::env::var_os("WHYCODES_TEST_CURRENT_EXE");
        let prev_fail = std::env::var_os("WHYCODES_TEST_CURRENT_EXE_FAIL");
        unsafe { std::env::remove_var("WHYCODES") };
        unsafe { std::env::remove_var("WHYCODES_TEST_CURRENT_EXE_FAIL") };

        let dir = tempfile::tempdir().unwrap();
        let sibling = dir.path().join("whycodes");
        std::fs::write(&sibling, b"").unwrap();
        let exe = dir.path().join("sdk-test");
        unsafe { std::env::set_var("WHYCODES_TEST_CURRENT_EXE", &exe) };
        assert_eq!(resolve_binary(None).unwrap(), sibling);

        let named = dir.path().join("whycodes");
        unsafe { std::env::set_var("WHYCODES_TEST_CURRENT_EXE", &named) };
        assert_eq!(resolve_binary(None).unwrap(), named);

        let empty_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(empty_dir.path().join("whycodes")).unwrap();
        let other = empty_dir.path().join("other-bin");
        unsafe { std::env::set_var("WHYCODES_TEST_CURRENT_EXE", &other) };
        assert_eq!(resolve_binary(None).unwrap(), PathBuf::from("whycodes"));

        let no_stem = empty_dir.path().join("..");
        unsafe { std::env::set_var("WHYCODES_TEST_CURRENT_EXE", &no_stem) };
        assert_eq!(resolve_binary(None).unwrap(), PathBuf::from("whycodes"));

        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let weird = empty_dir
                .path()
                .join(std::ffi::OsString::from_vec(vec![0xff]));
            unsafe { std::env::set_var("WHYCODES_TEST_CURRENT_EXE", &weird) };
            assert_eq!(resolve_binary(None).unwrap(), PathBuf::from("whycodes"));
        }

        unsafe { std::env::set_var("WHYCODES_TEST_CURRENT_EXE", "") };
        let fallback = resolve_binary(None).unwrap();
        assert!(fallback.ends_with("whycodes"));

        unsafe { std::env::set_var("WHYCODES_TEST_CURRENT_EXE_FAIL", "1") };
        assert_eq!(resolve_binary(None).unwrap(), PathBuf::from("whycodes"));

        match prev_why {
            Some(v) => unsafe { std::env::set_var("WHYCODES", v) },
            None => unsafe { std::env::remove_var("WHYCODES") },
        }
        match prev_exe {
            Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_CURRENT_EXE", v) },
            None => unsafe { std::env::remove_var("WHYCODES_TEST_CURRENT_EXE") },
        }
        match prev_fail {
            Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_CURRENT_EXE_FAIL", v) },
            None => unsafe { std::env::remove_var("WHYCODES_TEST_CURRENT_EXE_FAIL") },
        }
    }

    fn write_fake_binary(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("whycodes");
        let script = format!("#!/usr/bin/env python3\n{body}\n");
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    const PY_HEALTH: &str = r#"
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
PORT = int(sys.argv[2])
PROTO = int(__import__("os").environ.get("FAKE_PROTO", "1"))
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.split("?", 1)[0] == "/v1/health":
            body = json.dumps({
                "protocol": PROTO,
                "version": "test",
                "healthy": True,
                "project": "/tmp",
                "uptime_secs": 1,
                "sessions_in_memory": 0,
            }).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        else:
            self.send_response(404)
            self.end_headers()
    def log_message(self, *args):
        pass
HTTPServer(("127.0.0.1", PORT), H).serve_forever()
"#;

    #[tokio::test]
    async fn event_stream_covers_poll_states() {
        struct OncePendingThenNone {
            n: u8,
        }
        impl futures::Stream for OncePendingThenNone {
            type Item = reqwest::Result<Vec<u8>>;
            fn poll_next(
                mut self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Option<Self::Item>> {
                if self.n == 0 {
                    self.n = 1;
                    cx.waker().wake_by_ref();
                    return std::task::Poll::Pending;
                }
                std::task::Poll::Ready(None)
            }
        }

        let mut pending = EventStream {
            bytes: Box::pin(OncePendingThenNone { n: 0 }),
            buf: String::new(),
            done: false,
        };
        assert!(pending.next().await.is_none());

        let mut already_done = EventStream {
            bytes: futures::stream::empty().boxed(),
            buf: String::new(),
            done: true,
        };
        assert!(already_done.next().await.is_none());

        let mut bad = EventStream {
            bytes: futures::stream::iter([Ok(b"data: not-json\n\n".to_vec())]).boxed(),
            buf: String::new(),
            done: false,
        };
        let err = bad.next().await.unwrap().unwrap_err();
        assert_eq!(err.code, ErrorCode::Internal);

        let mut crlf = EventStream {
            bytes: futures::stream::iter([Ok(b"data: {\"ev\":\"cancelled\"}\r\n\r\n".to_vec())])
                .boxed(),
            buf: String::new(),
            done: false,
        };
        assert!(matches!(
            crlf.next().await.unwrap().unwrap(),
            SdkEvent::Cancelled
        ));

        let mut split = EventStream {
            bytes: futures::stream::iter([
                Ok(b"data: {\"ev\":\"tur".to_vec()),
                Ok(b"n_done\",\"text\":\"x\"}\n\n".to_vec()),
            ])
            .boxed(),
            buf: String::new(),
            done: false,
        };
        match split.next().await.unwrap().unwrap() {
            SdkEvent::TurnDone { text } => assert_eq!(text, "x"),
            other => panic!("{other:?}"),
        }
        assert!(split.next().await.is_none());
    }

    #[tokio::test]
    async fn take_stderr_empty_without_child() {
        let mut none = None;
        assert!(take_stderr(&mut none).await.is_empty());
    }

    #[tokio::test]
    async fn take_stderr_reads_child_output_and_missing_pipe() {
        let mut none_pipe = Some(Command::new("true").stderr(Stdio::null()).spawn().unwrap());
        assert!(take_stderr(&mut none_pipe).await.is_empty());

        let mut empty = Some(Command::new("true").stderr(Stdio::piped()).spawn().unwrap());
        let _ = empty.as_mut().unwrap().wait().await;
        assert!(take_stderr(&mut empty).await.is_empty());

        let mut noisy = Some(
            Command::new("sh")
                .arg("-c")
                .arg("echo boom >&2")
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let _ = noisy.as_mut().unwrap().wait().await;
        let text = take_stderr(&mut noisy).await;
        assert!(text.contains("boom"), "{text}");
    }

    #[tokio::test]
    async fn close_and_drop_kill_launched_child() {
        let live = Command::new("sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let client = WhyCodesClient::unconnected("http://127.0.0.1:9", http_client().unwrap())
            .with_child(live);
        client.close().await.unwrap();

        let live = Command::new("sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        drop(
            WhyCodesClient::unconnected("http://127.0.0.1:9", http_client().unwrap())
                .with_child(live),
        );

        let mut dead = Command::new("true").spawn().unwrap();
        let _ = dead.wait().await;
        drop(
            WhyCodesClient::unconnected("http://127.0.0.1:9", http_client().unwrap())
                .with_child(dead),
        );

        let mut dead = Command::new("true").spawn().unwrap();
        let _ = dead.wait().await;
        WhyCodesClient::unconnected("http://127.0.0.1:9", http_client().unwrap())
            .with_child(dead)
            .close()
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn launch_missing_binary_is_serve_not_found() {
        let err = match WhyCodesClient::launch(LaunchOptions {
            binary: Some(PathBuf::from("/no/such/whycodes-binary")),
            inherit_logins: false,
            startup_timeout: Duration::from_millis(200),
            ..Default::default()
        })
        .await
        {
            Err(e) => e,
            Ok(_) => panic!("expected ServeNotFound"),
        };
        assert_eq!(err.code, ErrorCode::ServeNotFound);
    }

    #[tokio::test]
    async fn launch_isolated_home_and_tempdir_connect() {
        let dir = tempfile::tempdir().unwrap();
        let binary = write_fake_binary(dir.path(), PY_HEALTH);
        let home = dir.path().join("home");
        let client = WhyCodesClient::launch(LaunchOptions {
            working_dir: dir.path().to_path_buf(),
            binary: Some(binary.clone()),
            inherit_logins: false,
            home: Some(home.clone()),
            startup_timeout: Duration::from_secs(5),
            port: None,
        })
        .await
        .unwrap();
        assert!(home.is_dir());
        assert!(client.base_url().starts_with("http://127.0.0.1:"));
        client.close().await.unwrap();

        let inherit_home = dir.path().join("inherit-home");
        let client = WhyCodesClient::launch(LaunchOptions {
            working_dir: dir.path().to_path_buf(),
            binary: Some(binary.clone()),
            inherit_logins: true,
            home: Some(inherit_home.clone()),
            startup_timeout: Duration::from_secs(5),
            port: Some(ephemeral_port().unwrap()),
        })
        .await
        .unwrap();
        assert!(inherit_home.is_dir());
        client.close().await.unwrap();

        let client = WhyCodesClient::launch(LaunchOptions {
            working_dir: dir.path().to_path_buf(),
            binary: Some(binary),
            inherit_logins: false,
            home: None,
            startup_timeout: Duration::from_secs(5),
            port: Some(ephemeral_port().unwrap()),
        })
        .await
        .unwrap();
        client.close().await.unwrap();
    }

    #[tokio::test]
    async fn launch_inherited_logins_retries_until_healthy() {
        let dir = tempfile::tempdir().unwrap();
        let body = format!("import time\ntime.sleep(0.12)\n{PY_HEALTH}");
        let binary = write_fake_binary(dir.path(), &body);
        let client = WhyCodesClient::launch(LaunchOptions {
            working_dir: dir.path().to_path_buf(),
            binary: Some(binary),
            inherit_logins: true,
            home: None,
            startup_timeout: Duration::from_secs(5),
            port: Some(ephemeral_port().unwrap()),
        })
        .await
        .unwrap();
        client.close().await.unwrap();
    }

    #[tokio::test]
    async fn launch_unsupported_version_does_not_retry() {
        let dir = tempfile::tempdir().unwrap();
        let script = format!("import os\nos.environ['FAKE_PROTO']='99'\n{PY_HEALTH}");
        let err = match WhyCodesClient::launch(LaunchOptions {
            working_dir: dir.path().to_path_buf(),
            binary: Some(write_fake_binary(dir.path(), &script)),
            inherit_logins: false,
            home: Some(dir.path().join("home3")),
            startup_timeout: Duration::from_secs(5),
            port: Some(ephemeral_port().unwrap()),
        })
        .await
        {
            Err(e) => e,
            Ok(_) => panic!("expected UnsupportedVersion"),
        };
        assert_eq!(err.code, ErrorCode::UnsupportedVersion);
    }

    #[tokio::test]
    async fn launch_child_exit_is_startup_failed() {
        let dir = tempfile::tempdir().unwrap();
        let binary = write_fake_binary(
            dir.path(),
            "import sys\nsys.stderr.write('nope\\n')\nraise SystemExit(7)\n",
        );
        let err = match WhyCodesClient::launch(LaunchOptions {
            working_dir: dir.path().to_path_buf(),
            binary: Some(binary),
            inherit_logins: false,
            home: None,
            startup_timeout: Duration::from_secs(3),
            port: Some(ephemeral_port().unwrap()),
        })
        .await
        {
            Err(e) => e,
            Ok(_) => panic!("expected StartupFailed"),
        };
        assert_eq!(err.code, ErrorCode::StartupFailed);
        assert!(err.message.contains("nope") || err.message.contains("exited"));
    }

    #[tokio::test]
    async fn launch_timeout_closes_stderr_then_hangs() {
        let dir = tempfile::tempdir().unwrap();
        let binary = write_fake_binary(
            dir.path(),
            "import os, sys, time\nsys.stderr.write('waiting\\n')\nsys.stderr.flush()\nos.close(2)\ntime.sleep(30)\n",
        );
        let err = match WhyCodesClient::launch(LaunchOptions {
            working_dir: dir.path().to_path_buf(),
            binary: Some(binary),
            inherit_logins: false,
            home: None,
            startup_timeout: Duration::from_millis(250),
            port: Some(ephemeral_port().unwrap()),
        })
        .await
        {
            Err(e) => e,
            Ok(_) => panic!("expected StartupTimeout"),
        };
        assert_eq!(err.code, ErrorCode::StartupTimeout);
        assert!(err.message.contains("waiting") || err.message.contains("did not become healthy"));
    }

    #[tokio::test]
    async fn launch_home_create_failure_is_startup_failed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("not-a-dir");
        std::fs::write(&file, b"x").unwrap();
        let err = match WhyCodesClient::launch(LaunchOptions {
            working_dir: dir.path().to_path_buf(),
            binary: Some(dir.path().join("unused")),
            inherit_logins: true,
            home: Some(file.join("nested")),
            startup_timeout: Duration::from_millis(200),
            port: Some(1),
        })
        .await
        {
            Err(e) => e,
            Ok(_) => panic!("expected StartupFailed"),
        };
        assert_eq!(err.code, ErrorCode::StartupFailed);
        assert!(err.message.contains("WHYCODES_HOME"));
    }
}
