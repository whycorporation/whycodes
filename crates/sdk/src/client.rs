//! HTTP client for protocol v1.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use futures::StreamExt;
use tokio::process::{Child, Command};
use whycode_protocol::sdk::{
    CreateSessionRequest, ErrorCode, Handshake, PROTOCOL_MAJOR, PermissionDecision,
    PermissionResponse, RunRequest, SdkEvent, SessionInfo, SessionList, StructuredAttempt,
    StructuredResult, ToolCallSummary, TurnResult, UsageSnapshot, extract_json, validate_instance,
    validate_schema,
};

use crate::SdkError;

/// Options for [`WhycodeClient::launch`].
#[derive(Debug, Clone)]
pub struct LaunchOptions {
    pub working_dir: PathBuf,
    /// Bind port. `None` picks an ephemeral loopback port.
    pub port: Option<u16>,
    /// `whycode` binary. Falls back to `$WHYCODE`, then PATH.
    pub binary: Option<PathBuf>,
    pub startup_timeout: Duration,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            port: None,
            binary: None,
            startup_timeout: Duration::from_secs(15),
        }
    }
}

/// Options for [`WhycodeClient::run`].
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub max_turns: Option<usize>,
    /// When `None`, [`WhycodeClient::run`] defaults to `true` (scripts) and
    /// [`WhycodeClient::run_events`] defaults to `false` (interactive UIs).
    pub auto_approve: Option<bool>,
}

/// Connection to a `whycode serve` process.
pub struct WhycodeClient {
    base: String,
    http: reqwest::Client,
    child: Option<Child>,
}

impl WhycodeClient {
    /// Attach to an already-running daemon (`whycode serve`).
    pub async fn connect(base: impl AsRef<str>) -> Result<Self, SdkError> {
        let base = normalize_base(base.as_ref());
        let http = http_client()?;
        let client = Self {
            base,
            http,
            child: None,
        };
        client.handshake().await?;
        Ok(client)
    }

    /// Spawn `whycode serve` as a private instance and connect to it.
    ///
    /// The child inherits this process's environment (API keys, `HOME`), so
    /// it spends the same provider quota as the user. `close` / drop kills it.
    pub async fn launch(opts: LaunchOptions) -> Result<Self, SdkError> {
        let port = match opts.port {
            Some(p) => p,
            None => ephemeral_port()?,
        };
        let binary = resolve_binary(opts.binary.as_deref())?;
        let mut cmd = Command::new(&binary);
        cmd.arg("serve")
            .arg(port.to_string())
            .current_dir(&opts.working_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
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
                    format!("whycode serve exited ({status}). {stderr}"),
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
        for i in 0..=retries {
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
        }
        Err(SdkError::new(
            ErrorCode::StructuredOutputInvalid,
            "exhausted structured retries",
        ))
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
                    "{} has no /v1/health — upgrade whycode serve (need protocol {PROTOCOL_MAJOR})",
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

impl Drop for WhycodeClient {
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

fn resolve_binary(explicit: Option<&Path>) -> Result<PathBuf, SdkError> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    if let Ok(p) = std::env::var("WHYCODE")
        && !p.is_empty()
    {
        return Ok(PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe() {
        if exe
            .file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|n| n == "whycode")
        {
            return Ok(exe);
        }
        let sibling = exe.with_file_name(if cfg!(windows) {
            "whycode.exe"
        } else {
            "whycode"
        });
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    Ok(PathBuf::from("whycode"))
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
}
