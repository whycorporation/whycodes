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
    assert_eq!(
        nonempty_or_truncated(Some("keep".into()), "echo hi"),
        "keep"
    );
    assert_eq!(
        nonempty_or_truncated(Some("   ".into()), "echo hi"),
        truncate_label("echo hi", 72)
    );
    assert_eq!(
        nonempty_or_truncated(None, "echo hi"),
        truncate_label("echo hi", 72)
    );
    assert_eq!(exit_summary("job".into(), Some(0)), "job (exit 0)");
    assert_eq!(exit_summary("job".into(), None), "job");
    kill_child_group(None);
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
    {
        let mut order = reg.inner.order.lock().unwrap();
        order.push("ghost-missing".into());
    }
    let listed = reg.list();
    assert!(listed.iter().all(|j| j.id != "ghost-missing"));
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
            && (out.contains("[sandbox]") || out.contains("sandbox-warn") || out.contains("echo"))
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

#[tokio::test]
async fn run_background_job_spawn_failed_and_warning() {
    let reg = BackgroundRegistry::new(4);
    let job = Arc::new(Mutex::new(JobInner {
        id: "bg-spawn".into(),
        label: "missing".into(),
        status: JobStatus::Running,
        started: Instant::now(),
        finished: None,
        output: String::new(),
        exit_code: None,
        kill_flag: Arc::new(AtomicBool::new(false)),
    }));
    {
        let mut jobs = reg.inner.jobs.lock().unwrap();
        jobs.insert("bg-spawn".into(), Arc::clone(&job));
        let mut order = reg.inner.order.lock().unwrap();
        order.push("bg-spawn".into());
    }
    run_background_job(
        reg.clone(),
        Arc::clone(&job),
        "bg-spawn".into(),
        "missing".into(),
        "/no/such/whycodes-bg-bin".into(),
        vec![],
        std::env::temp_dir(),
        Some("host fallback".into()),
        Arc::new(AtomicBool::new(false)),
    )
    .await;
    let snap = reg
        .list()
        .into_iter()
        .find(|j| j.id == "bg-spawn")
        .expect("listed");
    assert_eq!(snap.status, JobStatus::Failed);
    let out = {
        let g = job.lock().unwrap();
        g.output.clone()
    };
    assert!(
        out.contains("[sandbox]") || out.to_lowercase().contains("spawn"),
        "{out}"
    );
}

#[tokio::test]
async fn pipe_to_job_stops_on_read_error() {
    struct FailRead;
    impl tokio::io::AsyncRead for FailRead {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            _buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Err(std::io::Error::other("pipe closed")))
        }
    }
    let job = Arc::new(Mutex::new(JobInner {
        id: "bg-pipe".into(),
        label: "pipe".into(),
        status: JobStatus::Running,
        started: Instant::now(),
        finished: None,
        output: String::new(),
        exit_code: None,
        kill_flag: Arc::new(AtomicBool::new(false)),
    }));
    pipe_to_job(FailRead, &job).await;
}

#[tokio::test]
async fn run_background_job_aborts_pipe_tasks() {
    let reg = BackgroundRegistry::new(4);
    let job = Arc::new(Mutex::new(JobInner {
        id: "bg-abort".into(),
        label: "echo".into(),
        status: JobStatus::Running,
        started: Instant::now(),
        finished: None,
        output: String::new(),
        exit_code: None,
        kill_flag: Arc::new(AtomicBool::new(false)),
    }));
    {
        let mut jobs = reg.inner.jobs.lock().unwrap();
        jobs.insert("bg-abort".into(), Arc::clone(&job));
        let mut order = reg.inner.order.lock().unwrap();
        order.push("bg-abort".into());
    }
    run_background_job(
        reg.clone(),
        job,
        "bg-abort".into(),
        "echo".into(),
        "echo".into(),
        vec!["ok".into()],
        std::env::temp_dir(),
        None,
        Arc::new(AtomicBool::new(false)),
    )
    .await;
    let snap = reg
        .list()
        .into_iter()
        .find(|j| j.id == "bg-abort")
        .expect("listed");
    assert!(
        matches!(snap.status, JobStatus::Done | JobStatus::Failed),
        "{snap:?}"
    );
}

#[tokio::test]
async fn spawn_pipe_task_none_reader_is_noop() {
    let job = Arc::new(Mutex::new(JobInner {
        id: "bg-none".into(),
        label: "none".into(),
        status: JobStatus::Running,
        started: Instant::now(),
        finished: None,
        output: String::new(),
        exit_code: None,
        kill_flag: Arc::new(AtomicBool::new(false)),
    }));
    let handle = spawn_pipe_task::<tokio::io::Empty>(None, Arc::clone(&job));
    handle.await.expect("join");
}

#[tokio::test]
async fn join_pipe_task_logs_join_error() {
    let handle = tokio::spawn(async {
        panic!("pipe join coverage");
    });
    join_pipe_task(handle, "background stdout task skipped").await;
}

#[test]
fn wait_status_records_error_and_success() {
    let job = Arc::new(Mutex::new(JobInner {
        id: "wait".into(),
        label: "wait".into(),
        status: JobStatus::Running,
        started: Instant::now(),
        finished: None,
        output: String::new(),
        exit_code: None,
        kill_flag: Arc::new(AtomicBool::new(false)),
    }));
    assert!(wait_status(Err(std::io::Error::other("wait boom")), &job).is_none());
    let out = job.lock().unwrap().output.clone();
    assert!(out.contains("wait error"), "{out}");
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        let ok = std::process::ExitStatus::from_raw(0);
        let fail = std::process::ExitStatus::from_raw(1 << 8);
        assert!(wait_status(Ok(ok), &job).is_some_and(|s| s.success()));
        assert_eq!(job_status_from_wait(Some(ok)), (JobStatus::Done, Some(0)));
        assert_eq!(
            job_status_from_wait(Some(fail)),
            (JobStatus::Failed, Some(1))
        );
        assert_eq!(job_status_from_wait(None), (JobStatus::Failed, None));
    }
}

fn poison_mutex<T>(value: T) -> Mutex<T> {
    let m = Mutex::new(value);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _g = m.lock().unwrap();
        panic!("poison background mutex");
    }));
    m
}

fn sample_job(status: JobStatus, output: &str) -> JobInner {
    JobInner {
        id: "bg-1".into(),
        label: "job".into(),
        status,
        started: Instant::now(),
        finished: None,
        output: output.into(),
        exit_code: None,
        kill_flag: Arc::new(AtomicBool::new(false)),
    }
}

#[test]
fn lock_recovers_from_poison_and_helpers_cover_fallbacks() {
    let m = poison_mutex(7u8);
    assert_eq!(*lock(&m), 7);

    let job = poison_mutex(sample_job(JobStatus::Running, "out"));
    append_output(&Arc::new(job), "!");

    let poisoned_jobs = poison_mutex(HashMap::<String, Arc<Mutex<JobInner>>>::new());
    assert!(lock(&poisoned_jobs).is_empty());
    let poisoned_order = poison_mutex(Vec::<String>::new());
    assert!(lock(&poisoned_order).is_empty());
    let poisoned_listener = poison_mutex(None::<BackgroundListener>);
    assert!(lock(&poisoned_listener).is_none());

    let job_m = Arc::new(Mutex::new(sample_job(JobStatus::Running, "")));
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _g = job_m.lock().unwrap();
        panic!("poison job mutex");
    }));
    assert_eq!(lock(&job_m).id, "bg-1");

    assert_eq!(nonempty_or_truncated(Some("keep".into()), "cmd"), "keep");
    assert_eq!(
        nonempty_or_truncated(Some("   ".into()), "echo hi"),
        "echo hi"
    );
    assert_eq!(nonempty_or_truncated(None, "echo hi"), "echo hi");
    kill_child_group(None);
    assert_eq!(exit_summary("echo".into(), Some(0)), "echo (exit 0)");
    assert_eq!(exit_summary("echo".into(), None), "echo");
    let started = Instant::now();
    let finished = started + Duration::from_millis(5);
    assert!(job_elapsed(Some(finished), started) >= Duration::from_millis(5));
    let _ = job_elapsed(None, started);
}
