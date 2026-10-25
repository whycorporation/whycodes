//! Background shell jobs for agent automation (FEATURES §11).
//!
//! Long-running shell commands can return immediately with a job id; output is
//! buffered and available via the `bg` tool / `/bg` slash. Completions notify
//! via an optional listener (`TurnEvent::Background`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use whycodes_core::SandboxSettings;
use whycodes_sandbox::{PreparedCommand, SandboxError, SandboxRequest, kill_pid_group, prepare};

/// Recover from a poisoned mutex instead of aborting (`panic = "abort"` in release).
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Soft cap on concurrent running jobs.
pub const DEFAULT_MAX_BACKGROUND_JOBS: usize = 8;

/// Keep at most this many bytes of combined stdout/stderr per job.
pub const MAX_JOB_OUTPUT_BYTES: usize = 64 * 1024;

/// Stop growing a job's scratch log at this size; memory keeps the tail.
pub const MAX_JOB_LOG_BYTES: u64 = 64 * 1024 * 1024;

/// Finished jobs retained for `/bg` / `bg list` after exit.
const RETAIN_FINISHED: usize = 16;

/// After the shell exits, keep reading its pipes this long. A process it left
/// running (`server &`) holds them open, and EOF would never come.
const PIPE_DRAIN_GRACE: Duration = Duration::from_secs(2);

const PIPE_HELD_NOTE: &str =
    "\n[stopped reading output: a process this command started still holds it open]\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Running,
    Done,
    Failed,
    Killed,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Killed => "killed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct BackgroundEvent {
    pub id: String,
    pub status: JobStatus,
    pub summary: String,
}

pub type BackgroundListener = Arc<dyn Fn(BackgroundEvent) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct JobSnapshot {
    pub id: String,
    pub label: String,
    pub status: JobStatus,
    pub elapsed: Duration,
    pub output_len: usize,
    pub exit_code: Option<i32>,
}

struct JobInner {
    id: String,
    label: String,
    status: JobStatus,
    started: Instant,
    finished: Option<Instant>,
    output: String,
    exit_code: Option<i32>,
    kill_flag: Arc<AtomicBool>,
    /// Shell pid (= its process group) while it runs and is not yet reaped,
    /// so `kill` can signal the group without waiting for the job task.
    pid: Option<u32>,
    /// Windows has no process groups: the shell's Job Object, same lifetime
    /// as `pid`, so `kill` reaches the processes it started.
    #[cfg(windows)]
    tree: Option<whycodes_sandbox::ProcessTree>,
    /// Auto-detached foreground `bash` (issue #140). Completion is delivered once.
    auto_background: bool,
    completion_delivered: bool,
    /// Detached stdout/stderr under `.whycodes/scratch/<id>.log`.
    log: Option<ScratchLog>,
}

/// Shared registry of background shell jobs for one agent/session.
#[derive(Clone)]
pub struct BackgroundRegistry {
    inner: Arc<RegistryInner>,
}

struct RegistryInner {
    next_id: AtomicU64,
    jobs: Mutex<HashMap<String, Arc<Mutex<JobInner>>>>,
    /// Order of job ids for list (newest last).
    order: Mutex<Vec<String>>,
    max_jobs: AtomicU64,
    listener: Mutex<Option<BackgroundListener>>,
    /// Woken on every finish or kill (each goes through `emit`).
    changed: tokio::sync::Notify,
}

impl std::fmt::Debug for BackgroundRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.running_count();
        f.debug_struct("BackgroundRegistry")
            .field("running", &n)
            .finish()
    }
}

impl Default for BackgroundRegistry {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_BACKGROUND_JOBS)
    }
}

impl BackgroundRegistry {
    pub fn new(max_jobs: usize) -> Self {
        Self {
            inner: Arc::new(RegistryInner {
                next_id: AtomicU64::new(1),
                jobs: Mutex::new(HashMap::new()),
                order: Mutex::new(Vec::new()),
                max_jobs: AtomicU64::new(max_jobs.max(1) as u64),
                listener: Mutex::new(None),
                changed: tokio::sync::Notify::new(),
            }),
        }
    }

    /// Update concurrent-job ceiling (e.g. after config load).
    pub fn set_max_jobs(&self, max_jobs: usize) {
        self.inner
            .max_jobs
            .store(max_jobs.max(1) as u64, Ordering::Relaxed);
    }

    pub fn max_jobs(&self) -> usize {
        self.inner.max_jobs.load(Ordering::Relaxed) as usize
    }

    pub fn set_listener(&self, listener: Option<BackgroundListener>) {
        *lock(&self.inner.listener) = listener;
    }

    pub fn running_count(&self) -> usize {
        let jobs = lock(&self.inner.jobs);
        jobs.values()
            .filter(|j| lock(j).status == JobStatus::Running)
            .count()
    }

    pub fn list(&self) -> Vec<JobSnapshot> {
        // `jobs` before `order`, as in `start_prepared`; the reverse deadlocks.
        let jobs = lock(&self.inner.jobs);
        let order = lock(&self.inner.order);
        let mut out = Vec::new();
        for id in order.iter() {
            let Some(j) = jobs.get(id) else {
                continue;
            };
            let g = lock(j);
            let elapsed = job_elapsed(g.finished, g.started);
            out.push(JobSnapshot {
                id: g.id.clone(),
                label: g.label.clone(),
                status: g.status,
                elapsed,
                output_len: g.output.len(),
                exit_code: g.exit_code,
            });
        }
        out
    }

    pub fn read(&self, id: &str, max_chars: usize) -> Result<String, String> {
        let jobs = lock(&self.inner.jobs);
        let job = jobs
            .get(id)
            .ok_or_else(|| format!("unknown background job `{id}`"))?;
        let g = lock(job);
        let text = &g.output;
        if max_chars == 0 || text.chars().count() <= max_chars {
            return Ok(format!(
                "[{} {}] {}\n{}",
                g.id,
                g.status.as_str(),
                g.label,
                text
            ));
        }
        let tail: String = text
            .chars()
            .rev()
            .take(max_chars)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        Ok(format!(
            "[{} {}] {}\n…(truncated)\n{}",
            g.id,
            g.status.as_str(),
            g.label,
            tail
        ))
    }

    pub fn kill(&self, id: &str) -> Result<String, String> {
        let jobs = lock(&self.inner.jobs);
        let job = jobs
            .get(id)
            .ok_or_else(|| format!("unknown background job `{id}`"))?
            .clone();
        drop(jobs);
        let mut g = lock(&job);
        if g.status != JobStatus::Running {
            return Ok(format!("job `{}` already {}", g.id, g.status.as_str()));
        }
        g.kill_flag.store(true, Ordering::SeqCst);
        // Signal the group now: at teardown the runtime may drop the job task
        // before it runs, and kill_on_drop reaches the shell only.
        kill_child_group(g.pid.take());
        #[cfg(windows)]
        if let Some(tree) = g.tree.take() {
            tree.kill();
        }
        // Wait loop in the spawn task observes the flag and start_kill()s the child.
        g.status = JobStatus::Killed;
        g.finished = Some(Instant::now());
        let id = g.id.clone();
        let label = g.label.clone();
        drop(g);
        self.emit(BackgroundEvent {
            id: id.clone(),
            status: JobStatus::Killed,
            summary: format!("killed: {label}"),
        });
        Ok(format!("killed background job `{id}`"))
    }

    /// Kill every running job (session teardown).
    pub fn kill_all(&self) {
        let ids: Vec<String> = self
            .list()
            .into_iter()
            .filter(|j| j.status == JobStatus::Running)
            .map(|j| j.id)
            .collect();
        for id in ids {
            note_kill_result(self.kill(&id), &id);
        }
    }

    /// Start a shell command in the background. Returns job id.
    pub fn start_shell(
        &self,
        command: &str,
        working_dir: PathBuf,
        sandbox: SandboxSettings,
        label: Option<String>,
    ) -> Result<String, String> {
        self.start_shell_tagged(command, working_dir, sandbox, label, false)
    }

    /// Start a shell that was auto-detached from a foreground `bash`.
    pub fn start_shell_auto(
        &self,
        command: &str,
        working_dir: PathBuf,
        sandbox: SandboxSettings,
        label: Option<String>,
    ) -> Result<String, String> {
        self.start_shell_tagged(command, working_dir, sandbox, label, true)
    }

    fn start_shell_tagged(
        &self,
        command: &str,
        working_dir: PathBuf,
        sandbox: SandboxSettings,
        label: Option<String>,
        auto_background: bool,
    ) -> Result<String, String> {
        let max = self.max_jobs();
        if self.running_count() >= max {
            return Err(format!(
                "too many background jobs (max {max}); kill one with `bg` action=kill"
            ));
        }

        let request = SandboxRequest {
            command: command.to_string(),
            working_dir,
            settings: sandbox,
        };
        self.start_prepared(command, label, prepare_job(&request), auto_background)
    }

    /// Wait until `id` leaves `Running` or `timeout` elapses. Does not observe
    /// other jobs — a different completion cannot detach this wait.
    pub async fn wait_until_idle(&self, id: &str, timeout: Duration) -> JobStatus {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            // Registered before the status read, so a finish in between still wakes us.
            let changed = self.inner.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let status = {
                let jobs = lock(&self.inner.jobs);
                jobs.get(id).map(|j| lock(j).status)
            };
            match status {
                None => return JobStatus::Failed,
                Some(JobStatus::Running) => {
                    if tokio::time::timeout_at(deadline, changed).await.is_err() {
                        return JobStatus::Running;
                    }
                }
                Some(st) => return st,
            }
        }
    }

    /// Return when any job finishes or is killed, or after `max`.
    pub async fn changed_within(&self, max: Duration) {
        drop(tokio::time::timeout(max, self.inner.changed.notified()).await);
    }

    /// Last lines of output for a running handle (truncated).
    pub fn tail(&self, id: &str, max_chars: usize) -> String {
        self.read(id, max_chars).unwrap_or_default()
    }

    /// Finished auto-background jobs that have not been delivered to the model.
    pub fn take_auto_completions(&self) -> Vec<(String, JobStatus, String)> {
        let jobs = lock(&self.inner.jobs);
        let mut out = Vec::new();
        for job in jobs.values() {
            let mut g = lock(job);
            if !g.auto_background || g.completion_delivered || g.status == JobStatus::Running {
                continue;
            }
            g.completion_delivered = true;
            out.push((g.id.clone(), g.status, g.output.clone()));
        }
        out
    }

    /// Foreground auto-bg that finished before the idle timeout: do not inject
    /// the same output again on the next turn.
    pub fn mark_auto_delivered(&self, id: &str) {
        let jobs = lock(&self.inner.jobs);
        if let Some(job) = jobs.get(id) {
            lock(job).completion_delivered = true;
        }
    }

    #[allow(clippy::question_mark)]
    fn start_prepared(
        &self,
        command: &str,
        label: Option<String>,
        prepared: Result<PreparedCommand, String>,
        auto_background: bool,
    ) -> Result<String, String> {
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(e) => return Err(e),
        };

        let (id, log) = claim_job_log(&self.inner.next_id, &prepared.working_dir);
        let label = nonempty_or_truncated(label, command);
        let kill_flag = Arc::new(AtomicBool::new(false));

        let job = Arc::new(Mutex::new(JobInner {
            id: id.clone(),
            label: label.clone(),
            status: JobStatus::Running,
            started: Instant::now(),
            finished: None,
            output: String::new(),
            exit_code: None,
            kill_flag: Arc::clone(&kill_flag),
            pid: None,
            #[cfg(windows)]
            tree: None,
            auto_background,
            completion_delivered: false,
            log,
        }));

        {
            let mut jobs = lock(&self.inner.jobs);
            jobs.insert(id.clone(), Arc::clone(&job));
            let mut order = lock(&self.inner.order);
            order.push(id.clone());
            // Prune old finished beyond retain count.
            self.prune_locked(&mut jobs, &mut order);
        }

        let reg = self.clone();
        let job_for_task = Arc::clone(&job);
        let id_for_task = id.clone();
        let label_for_task = label.clone();
        let program = prepared.program.clone();
        let args = prepared.args.clone();
        let cwd = prepared.working_dir.clone();
        let env = prepared.env.clone();
        let warning = prepared.warning.clone();

        tokio::spawn(async move {
            run_background_job(
                reg,
                job_for_task,
                id_for_task,
                label_for_task,
                program,
                args,
                cwd,
                env,
                warning,
                kill_flag,
            )
            .await;
        });

        Ok(id)
    }

    fn emit(&self, ev: BackgroundEvent) {
        self.inner.changed.notify_waiters();
        if let Some(ref f) = *lock(&self.inner.listener) {
            f(ev);
        }
    }

    #[cfg(test)]
    pub(crate) fn emit_for_test(&self, ev: BackgroundEvent) {
        self.emit(ev);
    }

    fn prune_locked(
        &self,
        jobs: &mut HashMap<String, Arc<Mutex<JobInner>>>,
        order: &mut Vec<String>,
    ) {
        let finished: Vec<String> = order
            .iter()
            .filter(|id| {
                jobs.get(id.as_str())
                    .map(|j| lock(j).status != JobStatus::Running)
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        if finished.len() <= RETAIN_FINISHED {
            return;
        }
        let drop_n = finished.len() - RETAIN_FINISHED;
        for id in finished.into_iter().take(drop_n) {
            jobs.remove(&id);
            order.retain(|x| x != &id);
        }
    }
}

fn truncate_label(s: &str, max: usize) -> String {
    let t = s.trim().replace('\n', " ");
    if t.chars().count() <= max {
        return t;
    }
    let kept: String = t.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

#[allow(clippy::too_many_arguments)]
async fn run_background_job(
    reg: BackgroundRegistry,
    job: Arc<Mutex<JobInner>>,
    id: String,
    label: String,
    program: String,
    args: Vec<String>,
    cwd: PathBuf,
    env: Vec<(String, std::ffi::OsString)>,
    warning: Option<String>,
    kill_flag: Arc<AtomicBool>,
) {
    // The no-bubblewrap fallback warning is the same for every job; the model
    // needs it once per process, like foreground shell output.
    if let Some(ref w) = warning
        && whycodes_sandbox::first_sighting(w)
    {
        append_output(&job, &format!("[sandbox] {w}\n"));
    }

    let mut cmd = tokio::process::Command::new(&program);
    cmd.args(&args)
        .current_dir(&cwd)
        .envs(env)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    whycodes_core::secret_env::strip_secret_env_vars(|name| {
        cmd.env_remove(name);
    });
    #[cfg(unix)]
    {
        // Own process group so `bg` kill / drop reaps grandchildren.
        cmd.process_group(0);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            finalize_job(
                &reg,
                &job,
                &id,
                JobStatus::Failed,
                None,
                &format!("spawn failed: {e}"),
            );
            return;
        }
    };

    lock(&job).pid = child.id();
    #[cfg(windows)]
    {
        lock(&job).tree = child
            .raw_handle()
            .and_then(whycodes_sandbox::ProcessTree::adopt);
    }
    let out_task = spawn_pipe_task(boxed_reader(child.stdout.take()), Arc::clone(&job));
    let err_task = spawn_pipe_task(boxed_reader(child.stderr.take()), Arc::clone(&job));

    let status = loop {
        if kill_flag.load(Ordering::SeqCst) {
            kill_child_group(child.id());
            // Kill/wait errors are best-effort: the child may already have exited.
            let kill = child.start_kill();
            let reaped = child.wait().await;
            drop((kill, reaped));
            break None;
        }
        match tokio::time::timeout(Duration::from_millis(200), child.wait()).await {
            Ok(wait) => break wait_status(wait, &job),
            Err(_timeout) => continue,
        }
    };
    // Reaped: the pid may be reused, so `kill` must not signal it any more.
    lock(&job).pid = None;
    #[cfg(windows)]
    {
        lock(&job).tree = None;
    }

    let drain_until = tokio::time::Instant::now() + PIPE_DRAIN_GRACE;
    let out_held = join_pipe_task(out_task, drain_until, "background stdout task skipped").await;
    let err_held = join_pipe_task(err_task, drain_until, "background stderr task skipped").await;
    if out_held || err_held {
        append_output(&job, PIPE_HELD_NOTE);
    }

    if kill_flag.load(Ordering::SeqCst) {
        finalize_job(&reg, &job, &id, JobStatus::Killed, None, &label);
        return;
    }

    let (st, code) = job_status_from_wait(status);
    let summary = exit_summary(label, code);
    finalize_job(&reg, &job, &id, st, code, &summary);
}

fn job_elapsed(finished: Option<Instant>, started: Instant) -> Duration {
    match finished {
        Some(f) => f.saturating_duration_since(started),
        None => started.elapsed(),
    }
}

fn nonempty_or_truncated(label: Option<String>, command: &str) -> String {
    match label {
        Some(s) if !s.trim().is_empty() => s,
        _ => truncate_label(command, 72),
    }
}

fn kill_child_group(pid: Option<u32>) {
    if let Some(pid) = pid {
        kill_pid_group(pid);
    }
}

fn note_kill_result(result: Result<String, String>, id: &str) {
    match result {
        Ok(_) => {}
        Err(e) => tracing::debug!(error = %e, %id, "background kill skipped"),
    }
}

fn exit_summary(label: String, code: Option<i32>) -> String {
    match code {
        Some(c) => format!("{label} (exit {c})"),
        None => label,
    }
}

fn wait_status(
    wait: Result<std::process::ExitStatus, std::io::Error>,
    job: &Arc<Mutex<JobInner>>,
) -> Option<std::process::ExitStatus> {
    match wait {
        Ok(st) => Some(st),
        Err(e) => {
            append_output(job, &format!("\nwait error: {e}\n"));
            None
        }
    }
}

fn job_status_from_wait(status: Option<std::process::ExitStatus>) -> (JobStatus, Option<i32>) {
    match status {
        Some(s) if s.success() => (JobStatus::Done, s.code()),
        Some(s) => (JobStatus::Failed, s.code()),
        None => (JobStatus::Failed, None),
    }
}

fn prepare_job(request: &SandboxRequest) -> Result<PreparedCommand, String> {
    sandbox_prepare_result(prepare(request))
}

fn sandbox_prepare_result(
    result: Result<PreparedCommand, SandboxError>,
) -> Result<PreparedCommand, String> {
    match result {
        Ok(prepared) => Ok(prepared),
        Err(e) => Err(e.to_string()),
    }
}

fn append_output(job: &Arc<Mutex<JobInner>>, chunk: &str) {
    let mut g = lock(job);
    g.output.push_str(chunk);
    cap_job_output(&mut g.output);
    if let Some(log) = g.log.as_mut() {
        log.append(chunk);
    }
}

/// A job's `.whycodes/scratch/<id>.log`, opened once: reopening it and
/// reading its length for every chunk dominated noisy commands on Windows.
struct ScratchLog {
    file: std::fs::File,
    len: u64,
}

impl ScratchLog {
    fn append(&mut self, chunk: &str) {
        if self.len >= MAX_JOB_LOG_BYTES {
            return;
        }
        match std::io::Write::write_all(&mut self.file, chunk.as_bytes()) {
            Ok(()) => self.len += chunk.len() as u64,
            Err(e) => tracing::debug!(error = %e, "background scratch write skipped"),
        }
    }
}

/// Takes the next job id whose log this call creates itself. Two sessions in
/// one project share `.whycodes/scratch/`: an id another one already logs
/// under is skipped instead of truncating its log.
fn claim_job_log(next_id: &AtomicU64, working_dir: &Path) -> (String, Option<ScratchLog>) {
    let dir = whycodes_core::project_scratch_dir(working_dir);
    loop {
        let id = format!("bg-{}", next_id.fetch_add(1, Ordering::SeqCst));
        let opened = std::fs::create_dir_all(&dir).and_then(|()| {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(dir.join(format!("{id}.log")))
        });
        match opened {
            Ok(file) => return (id, Some(ScratchLog { file, len: 0 })),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && dir.is_dir() => continue,
            Err(e) => {
                tracing::debug!(error = %e, "background scratch log skipped");
                return (id, None);
            }
        }
    }
}

fn cap_job_output(output: &mut String) {
    if output.len() <= MAX_JOB_OUTPUT_BYTES {
        return;
    }
    // Cut on a char boundary: draining inside one panics (abort in release).
    let mut excess = output.len() - MAX_JOB_OUTPUT_BYTES;
    while !output.is_char_boundary(excess) {
        excess += 1;
    }
    output.drain(..excess);
    prefix_ellipsis(output);
}

fn prefix_ellipsis(output: &mut String) {
    if output.starts_with('…') {
        return;
    }
    output.insert(0, '…');
}

type JobReader = Box<dyn tokio::io::AsyncRead + Unpin + Send>;

fn boxed_reader<R>(reader: Option<R>) -> Option<JobReader>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    reader.map(|r| Box::new(r) as JobReader)
}

fn spawn_pipe_task(
    reader: Option<JobReader>,
    job: Arc<Mutex<JobInner>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Some(out) = reader {
            pipe_to_job(out, &job).await;
        }
    })
}

/// Join a pipe reader by `deadline`; past it, abort the reader and return
/// true (something still holds the pipe open).
async fn join_pipe_task(
    mut task: tokio::task::JoinHandle<()>,
    deadline: tokio::time::Instant,
    skipped: &'static str,
) -> bool {
    match tokio::time::timeout_at(deadline, &mut task).await {
        Ok(joined) => {
            if let Err(e) = joined {
                let error = e.to_string();
                tracing::debug!(error = %error, "{skipped}");
            }
            false
        }
        Err(_elapsed) => {
            task.abort();
            true
        }
    }
}

async fn pipe_to_job(mut reader: JobReader, job: &Arc<Mutex<JobInner>>) {
    use tokio::io::AsyncReadExt;
    let mut buf = [0u8; 4096];
    // A read can end inside a multibyte char; its first bytes wait here for
    // the rest instead of turning into U+FFFD.
    let mut pending = Vec::new();
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                pending.extend_from_slice(&buf[..n]);
                let complete = complete_utf8_len(&pending);
                append_output(job, &String::from_utf8_lossy(&pending[..complete]));
                pending.drain(..complete);
            }
            Err(_read) => break,
        }
    }
    if !pending.is_empty() {
        append_output(job, &String::from_utf8_lossy(&pending));
    }
}

/// Length of `bytes` minus an incomplete UTF-8 sequence at its end. Invalid
/// bytes count as complete (they decode lossily right away).
fn complete_utf8_len(bytes: &[u8]) -> usize {
    match std::str::from_utf8(bytes) {
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        _complete_or_invalid => bytes.len(),
    }
}

fn finalize_job(
    reg: &BackgroundRegistry,
    job: &Arc<Mutex<JobInner>>,
    id: &str,
    status: JobStatus,
    exit_code: Option<i32>,
    summary: &str,
) {
    {
        let mut g = lock(job);
        // Don't overwrite Killed if already set by kill().
        if g.status == JobStatus::Killed {
            return;
        }
        g.status = status;
        g.exit_code = exit_code;
        g.finished = Some(Instant::now());
    }
    reg.emit(BackgroundEvent {
        id: id.to_string(),
        status,
        summary: summary.to_string(),
    });
}

#[cfg(test)]
#[path = "background_tests.rs"]
mod tests;
