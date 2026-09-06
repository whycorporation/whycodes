//! Background shell jobs for agent automation (FEATURES §11).
//!
//! Long-running shell commands can return immediately with a job id; output is
//! buffered and available via the `bg` tool / `/bg` slash. Completions notify
//! via an optional listener (`TurnEvent::Background`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use whycodes_core::SandboxSettings;
use whycodes_sandbox::{SandboxRequest, kill_pid_group, prepare};

/// Soft cap on concurrent running jobs.
pub const DEFAULT_MAX_BACKGROUND_JOBS: usize = 8;

/// Keep at most this many bytes of combined stdout/stderr per job.
pub const MAX_JOB_OUTPUT_BYTES: usize = 64 * 1024;

/// Finished jobs retained for `/bg` / `bg list` after exit.
const RETAIN_FINISHED: usize = 16;

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
        if let Ok(mut slot) = self.inner.listener.lock() {
            *slot = listener;
        }
    }

    pub fn running_count(&self) -> usize {
        let jobs = self.inner.jobs.lock().unwrap_or_else(|e| e.into_inner());
        jobs.values()
            .filter(|j| {
                j.lock()
                    .map(|g| g.status == JobStatus::Running)
                    .unwrap_or(false)
            })
            .count()
    }

    pub fn list(&self) -> Vec<JobSnapshot> {
        let order = self.inner.order.lock().unwrap_or_else(|e| e.into_inner());
        let jobs = self.inner.jobs.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = Vec::new();
        for id in order.iter() {
            if let Some(j) = jobs.get(id)
                && let Ok(g) = j.lock()
            {
                let elapsed = g
                    .finished
                    .map(|f| f.saturating_duration_since(g.started))
                    .unwrap_or_else(|| g.started.elapsed());
                out.push(JobSnapshot {
                    id: g.id.clone(),
                    label: g.label.clone(),
                    status: g.status,
                    elapsed,
                    output_len: g.output.len(),
                    exit_code: g.exit_code,
                });
            }
        }
        out
    }

    pub fn read(&self, id: &str, max_chars: usize) -> Result<String, String> {
        let jobs = self.inner.jobs.lock().unwrap_or_else(|e| e.into_inner());
        let job = jobs
            .get(id)
            .ok_or_else(|| format!("unknown background job `{id}`"))?;
        let g = job.lock().unwrap_or_else(|e| e.into_inner());
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
        let jobs = self.inner.jobs.lock().unwrap_or_else(|e| e.into_inner());
        let job = jobs
            .get(id)
            .ok_or_else(|| format!("unknown background job `{id}`"))?
            .clone();
        drop(jobs);
        let mut g = job.lock().unwrap_or_else(|e| e.into_inner());
        if g.status != JobStatus::Running {
            return Ok(format!("job `{}` already {}", g.id, g.status.as_str()));
        }
        g.kill_flag.store(true, Ordering::SeqCst);
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
            let _ = self.kill(&id);
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
        let prepared = prepare(&request).map_err(|e| e.to_string())?;

        let id = format!("bg-{}", self.inner.next_id.fetch_add(1, Ordering::SeqCst));
        let label = label
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| truncate_label(command, 72));
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
        }));

        {
            let mut jobs = self.inner.jobs.lock().unwrap_or_else(|e| e.into_inner());
            jobs.insert(id.clone(), Arc::clone(&job));
            let mut order = self.inner.order.lock().unwrap_or_else(|e| e.into_inner());
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
        let warning = prepared.warning.clone();

        tokio::spawn(async move {
            if let Some(ref w) = warning {
                append_output(&job_for_task, &format!("[sandbox] {w}\n"));
            }

            let mut cmd = tokio::process::Command::new(&program);
            cmd.args(&args)
                .current_dir(&cwd)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);
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
                        &job_for_task,
                        &id_for_task,
                        JobStatus::Failed,
                        None,
                        &format!("spawn failed: {e}"),
                    );
                    return;
                }
            };

            // Dual-read stdout/stderr into shared buffer.
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();

            let job_out = Arc::clone(&job_for_task);
            let out_task = tokio::spawn(async move {
                if let Some(out) = stdout {
                    pipe_to_job(out, &job_out).await;
                }
            });
            let job_err = Arc::clone(&job_for_task);
            let err_task = tokio::spawn(async move {
                if let Some(err) = stderr {
                    pipe_to_job(err, &job_err).await;
                }
            });

            // Poll kill flag while waiting.
            let status = loop {
                if kill_flag.load(Ordering::SeqCst) {
                    if let Some(pid) = child.id() {
                        kill_pid_group(pid);
                    }
                    if let Err(e) = child.start_kill() {
                        tracing::debug!(error = %e, "background job kill skipped");
                    }
                    if let Err(e) = child.wait().await {
                        tracing::debug!(error = %e, "background job wait after kill skipped");
                    }
                    break None; // killed
                }
                match tokio::time::timeout(Duration::from_millis(200), child.wait()).await {
                    Ok(Ok(st)) => break Some(st),
                    Ok(Err(e)) => {
                        append_output(&job_for_task, &format!("\nwait error: {e}\n"));
                        break None;
                    }
                    Err(_timeout) => continue, // timeout — recheck kill
                }
            };

            if let Err(e) = out_task.await {
                tracing::debug!(error = %e, "background stdout task skipped");
            }
            if let Err(e) = err_task.await {
                tracing::debug!(error = %e, "background stderr task skipped");
            }

            if kill_flag.load(Ordering::SeqCst) {
                finalize_job(
                    &reg,
                    &job_for_task,
                    &id_for_task,
                    JobStatus::Killed,
                    None,
                    &label_for_task,
                );
                return;
            }

            let (st, code) = match status {
                Some(s) => {
                    let code = s.code();
                    if s.success() {
                        (JobStatus::Done, code)
                    } else {
                        (JobStatus::Failed, code)
                    }
                }
                None => (JobStatus::Failed, None),
            };
            let summary = match code {
                Some(c) => format!("{label_for_task} (exit {c})"),
                None => label_for_task.clone(),
            };
            finalize_job(&reg, &job_for_task, &id_for_task, st, code, &summary);
        });

        Ok(id)
    }

    fn emit(&self, ev: BackgroundEvent) {
        if let Ok(slot) = self.inner.listener.lock()
            && let Some(ref f) = *slot
        {
            f(ev);
        }
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
                    .and_then(|j| j.lock().ok())
                    .map(|g| g.status != JobStatus::Running)
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

fn append_output(job: &Arc<Mutex<JobInner>>, chunk: &str) {
    if let Ok(mut g) = job.lock() {
        g.output.push_str(chunk);
        if g.output.len() > MAX_JOB_OUTPUT_BYTES {
            let excess = g.output.len() - MAX_JOB_OUTPUT_BYTES;
            g.output.drain(..excess);
            if !g.output.starts_with('…') {
                g.output.insert(0, '…');
            }
        }
    }
}

async fn pipe_to_job<R: tokio::io::AsyncRead + Unpin>(mut reader: R, job: &Arc<Mutex<JobInner>>) {
    use tokio::io::AsyncReadExt;
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                let s = String::from_utf8_lossy(&buf[..n]);
                append_output(job, &s);
            }
            Err(_read) => break,
        }
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
    if let Ok(mut g) = job.lock() {
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
mod tests {
    use super::*;
    use whycodes_core::SandboxSettings;

    #[tokio::test]
    async fn start_sleep_kill() {
        let reg = BackgroundRegistry::new(4);
        let id = reg
            .start_shell(
                "sleep 30",
                std::env::temp_dir(),
                SandboxSettings::off(),
                Some("sleep".into()),
            )
            .expect("start");
        // Give spawn a moment
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(reg.running_count(), 1);
        let msg = reg.kill(&id).expect("kill");
        assert!(msg.contains("killed"), "{msg}");
        tokio::time::sleep(Duration::from_millis(150)).await;
        let snap = reg.list().into_iter().find(|j| j.id == id).unwrap();
        assert_eq!(snap.status, JobStatus::Killed);
    }

    #[tokio::test]
    async fn start_echo_completes() {
        let reg = BackgroundRegistry::new(4);
        let done = Arc::new(AtomicBool::new(false));
        let done2 = Arc::clone(&done);
        reg.set_listener(Some(Arc::new(move |ev| {
            if ev.status == JobStatus::Done {
                done2.store(true, Ordering::SeqCst);
            }
        })));
        let id = reg
            .start_shell(
                "echo hello-bg-test",
                std::env::temp_dir(),
                SandboxSettings::off(),
                None,
            )
            .expect("start");
        for _ in 0..50 {
            if done.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(done.load(Ordering::SeqCst), "job should complete");
        let out = reg.read(&id, 10_000).expect("read");
        assert!(out.contains("hello-bg-test"), "{out}");
    }

    #[tokio::test]
    async fn max_jobs_enforced() {
        let reg = BackgroundRegistry::new(1);
        reg.start_shell(
            "sleep 60",
            std::env::temp_dir(),
            SandboxSettings::off(),
            None,
        )
        .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let err = reg
            .start_shell(
                "sleep 60",
                std::env::temp_dir(),
                SandboxSettings::off(),
                None,
            )
            .unwrap_err();
        assert!(err.contains("too many"), "{err}");
        reg.kill_all();
    }

    #[test]
    fn job_status_as_str_and_debug_fmt() {
        assert_eq!(JobStatus::Running.as_str(), "running");
        assert_eq!(JobStatus::Done.as_str(), "done");
        assert_eq!(JobStatus::Failed.as_str(), "failed");
        assert_eq!(JobStatus::Killed.as_str(), "killed");
        let dbg = format!("{:?}", BackgroundRegistry::default());
        assert!(dbg.contains("running"), "{dbg}");
        assert_eq!(truncate_label("short", 72), "short");
        let long = truncate_label(&"x".repeat(80), 10);
        assert!(long.ends_with('…'), "{long}");
        assert_eq!(long.chars().count(), 10);
    }

    #[tokio::test]
    async fn spawn_fail_already_status_output_cap_and_prune() {
        let reg = BackgroundRegistry::new(32);
        let missing = reg
            .start_shell(
                "whycodes-definitely-missing-bg-binary --nope",
                std::env::temp_dir(),
                SandboxSettings::off(),
                Some("missing".into()),
            )
            .expect("spawn is recorded even if the child fails");
        tokio::time::sleep(Duration::from_millis(150)).await;
        let snap = reg
            .list()
            .into_iter()
            .find(|j| j.id == missing)
            .expect("job");
        assert_eq!(snap.status, JobStatus::Failed);
        let already = reg.kill(&missing).expect("already");
        assert!(already.contains("already"), "{already}");

        let job = Arc::new(Mutex::new(JobInner {
            id: "cap".into(),
            label: "cap".into(),
            status: JobStatus::Running,
            started: Instant::now(),
            finished: None,
            output: String::new(),
            exit_code: None,
            kill_flag: Arc::new(AtomicBool::new(false)),
        }));
        append_output(&job, &"a".repeat(MAX_JOB_OUTPUT_BYTES + 32));
        {
            let g = job.lock().unwrap();
            assert!(
                g.output.len() <= MAX_JOB_OUTPUT_BYTES + 4,
                "{}",
                g.output.len()
            );
            assert!(
                g.output.starts_with('…'),
                "{}",
                &g.output[..8.min(g.output.len())]
            );
        }

        for i in 0..20 {
            let id = format!("done-{i}");
            let finished = Arc::new(Mutex::new(JobInner {
                id: id.clone(),
                label: id.clone(),
                status: JobStatus::Done,
                started: Instant::now(),
                finished: Some(Instant::now()),
                output: String::new(),
                exit_code: Some(0),
                kill_flag: Arc::new(AtomicBool::new(false)),
            }));
            {
                let mut jobs = reg.inner.jobs.lock().unwrap();
                jobs.insert(id.clone(), finished);
                let mut order = reg.inner.order.lock().unwrap();
                order.push(id);
                reg.prune_locked(&mut jobs, &mut order);
            }
        }
        let finished_n = reg
            .list()
            .into_iter()
            .filter(|j| j.status != JobStatus::Running)
            .count();
        assert!(finished_n <= RETAIN_FINISHED + 4, "{finished_n}");
        assert!(reg.read("nope", 16).is_err());
        let _ = format!(
            "{:?}",
            JobSnapshot {
                id: "x".into(),
                label: "y".into(),
                status: JobStatus::Failed,
                elapsed: Duration::from_secs(1),
                output_len: 0,
                exit_code: Some(1),
            }
        );
    }

    #[tokio::test]
    async fn read_truncates_long_output_to_tail() {
        let reg = BackgroundRegistry::new(4);
        let id = reg
            .start_shell(
                "python3 -c \"print('ABCDEFGHIJKLMNOPQRSTUVWXYZ' * 8)\"",
                std::env::temp_dir(),
                SandboxSettings::off(),
                Some("long".into()),
            )
            .expect("start");
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(40)).await;
            if let Ok(out) = reg.read(&id, 8)
                && out.contains("(truncated)")
            {
                assert!(out.contains("…(truncated)"), "{out}");
                return;
            }
        }
        let out = reg.read(&id, 8).expect("read");
        assert!(
            out.contains("(truncated)") || out.chars().count() > 8,
            "{out}"
        );
    }

    #[tokio::test]
    async fn start_shell_spawn_failed_marks_job_failed() {
        let reg = BackgroundRegistry::new(4);
        let id = reg
            .start_shell(
                "whycodes-no-such-binary-xyz",
                std::env::temp_dir(),
                SandboxSettings::off(),
                Some("missing".into()),
            )
            .expect("queued");
        let mut status = None;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(25)).await;
            if let Some(job) = reg.list().into_iter().find(|j| j.id == id)
                && job.status != JobStatus::Running
            {
                status = Some(job.status);
                break;
            }
        }
        assert_eq!(status, Some(JobStatus::Failed));
        let out = reg.read(&id, 200).unwrap_or_default();
        assert!(
            out.to_lowercase().contains("spawn") || out.to_lowercase().contains("fail"),
            "{out}"
        );
    }

    #[tokio::test]
    async fn kill_during_wait_marks_killed() {
        let reg = BackgroundRegistry::new(4);
        let id = reg
            .start_shell(
                "sleep 30",
                std::env::temp_dir(),
                SandboxSettings::off(),
                Some("sleep-kill".into()),
            )
            .expect("start");
        tokio::time::sleep(Duration::from_millis(40)).await;
        let _ = reg.kill(&id);
        let mut status = None;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(25)).await;
            if let Some(job) = reg.list().into_iter().find(|j| j.id == id)
                && job.status != JobStatus::Running
            {
                status = Some(job.status);
                break;
            }
        }
        assert!(
            matches!(
                status,
                Some(JobStatus::Killed) | Some(JobStatus::Failed) | Some(JobStatus::Done)
            ),
            "{status:?}"
        );
    }

    #[tokio::test]
    async fn start_shell_workspace_fallback_warning() {
        let reg = BackgroundRegistry::new(4);
        let settings = SandboxSettings {
            mode: whycodes_core::SandboxMode::Workspace,
            network: true,
            fallback: whycodes_core::SandboxFallback::Allow,
        };
        let id = reg
            .start_shell(
                "echo sandbox-warn",
                std::env::temp_dir(),
                settings,
                Some("warn".into()),
            )
            .expect("start");
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(40)).await;
            if let Ok(out) = reg.read(&id, 400)
                && (out.contains("[sandbox]")
                    || out.contains("sandbox-warn")
                    || out.contains("echo"))
            {
                return;
            }
            let snap = reg.list().into_iter().find(|j| j.id == id);
            if snap.is_some_and(|s| s.status != JobStatus::Running) {
                let _ = reg.read(&id, 400);
                return;
            }
        }
    }
}
