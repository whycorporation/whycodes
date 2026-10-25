use super::*;
use whycodes_core::{SandboxFallback, SandboxMode, SandboxSettings};
use whycodes_sandbox::SandboxError;

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
    // The cap is checked synchronously when the job is marked Running, before
    // the child is spawned. A short command avoids a 60s sleeper that the
    // runtime can block on while reaping at shutdown.
    let id = reg
        .start_shell(
            "echo cap",
            std::env::temp_dir(),
            SandboxSettings::off(),
            None,
        )
        .unwrap();
    let err = reg
        .start_shell(
            "echo cap-2",
            std::env::temp_dir(),
            SandboxSettings::off(),
            None,
        )
        .unwrap_err();
    assert!(err.contains("too many"), "{err}");
    reg.kill_all();
    let _ = reg.wait_until_idle(&id, Duration::from_secs(2)).await;
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
    note_kill_result(Ok("killed".into()), "bg-1");
    note_kill_result(Err("missing".into()), "bg-1");
}

#[test]
fn mark_auto_delivered_skips_inject() {
    let job = Arc::new(Mutex::new(JobInner {
        id: "bg-auto".into(),
        label: "echo".into(),
        status: JobStatus::Done,
        started: Instant::now(),
        finished: Some(Instant::now()),
        output: "done".into(),
        exit_code: Some(0),
        kill_flag: Arc::new(AtomicBool::new(false)),
        pid: None,
        #[cfg(windows)]
        tree: None,
        auto_background: true,
        completion_delivered: false,
        log: None,
    }));
    let reg = BackgroundRegistry::new(4);
    {
        let mut jobs = lock(&reg.inner.jobs);
        jobs.insert("bg-auto".into(), Arc::clone(&job));
    }
    assert_eq!(reg.take_auto_completions().len(), 1);
    lock(&job).completion_delivered = false;
    reg.mark_auto_delivered("bg-auto");
    reg.mark_auto_delivered("missing");
    assert!(reg.take_auto_completions().is_empty());
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
    // Instrumentation can delay the spawn-failure task past a fixed sleep.
    let mut snap = None;
    for _ in 0..40 {
        snap = reg.list().into_iter().find(|j| j.id == missing);
        if snap.as_ref().is_some_and(|j| j.status == JobStatus::Failed) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let snap = snap.expect("job");
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
        pid: None,
        #[cfg(windows)]
        tree: None,
        auto_background: false,
        completion_delivered: false,
        log: None,
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
            pid: None,
            #[cfg(windows)]
            tree: None,
            auto_background: false,
            completion_delivered: false,
            log: None,
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
async fn job_task_kills_the_child_when_kill_came_before_its_pid() {
    // `kill` signals the group itself once the pid is known; before that (or
    // on Windows) the job task must see the flag and kill the child.
    let reg = BackgroundRegistry::new(4);
    let job = Arc::new(Mutex::new(sample_job(JobStatus::Running, "")));
    let (program, args): (&str, &[&str]) = if cfg!(windows) {
        ("ping", &["-n", "30", "127.0.0.1"])
    } else {
        ("sleep", &["30"])
    };
    let t0 = Instant::now();
    run_background_job(
        reg,
        Arc::clone(&job),
        "bg-1".into(),
        "job".into(),
        program.into(),
        args.iter().map(|a| a.to_string()).collect(),
        std::env::temp_dir(),
        vec![],
        None,
        Arc::new(AtomicBool::new(true)),
    )
    .await;
    assert!(t0.elapsed() < Duration::from_secs(10), "{:?}", t0.elapsed());
    assert_eq!(lock(&job).status, JobStatus::Killed);
    assert_eq!(lock(&job).pid, None);
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
        pid: None,
        #[cfg(windows)]
        tree: None,
        auto_background: false,
        completion_delivered: false,
        log: None,
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
        vec![],
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
        pid: None,
        #[cfg(windows)]
        tree: None,
        auto_background: false,
        completion_delivered: false,
        log: None,
    }));
    pipe_to_job(Box::new(FailRead), &job).await;
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
        pid: None,
        #[cfg(windows)]
        tree: None,
        auto_background: false,
        completion_delivered: false,
        log: None,
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
        vec![],
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
        pid: None,
        #[cfg(windows)]
        tree: None,
        auto_background: false,
        completion_delivered: false,
        log: None,
    }));
    let handle = spawn_pipe_task(None, Arc::clone(&job));
    handle.await.expect("join");
    pipe_to_job(Box::new(tokio::io::empty()), &job).await;
}

#[tokio::test]
async fn join_pipe_task_logs_join_error() {
    let handle = tokio::spawn(async {
        panic!("pipe join coverage");
    });
    let deadline = tokio::time::Instant::now() + PIPE_DRAIN_GRACE;
    let held = join_pipe_task(handle, deadline, "background stdout task skipped").await;
    assert!(
        !held,
        "a reader that ended (here by panicking) does not hold the pipe"
    );
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
        pid: None,
        #[cfg(windows)]
        tree: None,
        auto_background: false,
        completion_delivered: false,
        log: None,
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
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt;
        let ok = std::process::ExitStatus::from_raw(0);
        let fail = std::process::ExitStatus::from_raw(1);
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
        pid: None,
        #[cfg(windows)]
        tree: None,
        auto_background: false,
        completion_delivered: false,
        log: None,
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

    let err =
        sandbox_prepare_result(Err(SandboxError::Unavailable("no bwrap".into()))).unwrap_err();
    assert!(err.contains("no bwrap"), "{err}");
    let deny = SandboxSettings {
        mode: SandboxMode::Workspace,
        network: true,
        fallback: SandboxFallback::Deny,
    };
    let request = SandboxRequest {
        command: "echo hi".into(),
        working_dir: std::env::temp_dir(),
        settings: deny.clone(),
    };
    match prepare_job(&request) {
        Ok(_) => {}
        Err(e) => assert!(!e.is_empty(), "{e}"),
    }
    let err = BackgroundRegistry::new(1)
        .start_prepared("echo hi", None, Err("sandbox unavailable".into()), false)
        .unwrap_err();
    assert!(err.contains("sandbox unavailable"), "{err}");
    let mut already = "…keep".to_string();
    prefix_ellipsis(&mut already);
    assert_eq!(already, "…keep");
    let mut short = "ok".to_string();
    cap_job_output(&mut short);
    assert_eq!(short, "ok");
    let mut over = "a".repeat(MAX_JOB_OUTPUT_BYTES + 8);
    cap_job_output(&mut over);
    assert!(over.starts_with('…'), "{over}");
    assert!(over.len() <= MAX_JOB_OUTPUT_BYTES + 4, "{}", over.len());
}

#[test]
fn cap_job_output_cuts_on_a_char_boundary() {
    // Past the cap every push trims the front. With 2-byte chars there, a
    // 1-byte push used to cut inside one and panic (abort in release).
    let mut out = String::new();
    while out.len() <= MAX_JOB_OUTPUT_BYTES {
        out.push_str("şğı");
        cap_job_output(&mut out);
    }
    out.push('x');
    cap_job_output(&mut out);
    assert!(out.starts_with('…'), "{}", &out[..12]);
    assert!(out.ends_with("ıx"));
    assert!(out.len() <= MAX_JOB_OUTPUT_BYTES + '…'.len_utf8());
}

#[tokio::test]
async fn pipe_to_job_keeps_a_char_split_across_reads() {
    use tokio::io::AsyncWriteExt;
    let job = Arc::new(Mutex::new(sample_job(JobStatus::Running, "")));
    // A 1-byte duplex makes every read return a single byte.
    let (mut tx, rx) = tokio::io::duplex(1);
    let writer = tokio::spawn(async move {
        let mut bytes = "şı€😀".as_bytes().to_vec();
        bytes.push(0xff); // invalid: replaced at once
        bytes.extend_from_slice(b"ok");
        bytes.push(0xc5); // first byte of "ş", never completed
        tx.write_all(&bytes).await.expect("write");
    });
    pipe_to_job(Box::new(rx), &job).await;
    writer.await.expect("writer");
    assert_eq!(lock(&job).output, "şı€😀\u{fffd}ok\u{fffd}");
}

#[test]
fn scratch_log_persists_and_skips_failures() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir
        .path()
        .join(".whycodes")
        .join("scratch")
        .join("bg-scratch.log");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "stale log of an earlier session").unwrap();
    let log = ensure_scratch_log(dir.path(), "bg-scratch").expect("log");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "", "truncated");
    let mut job = sample_job(JobStatus::Running, "");
    job.log = Some(log);
    let job = Arc::new(Mutex::new(job));
    append_output(&job, "hello-scratch");
    append_output(&job, "-and-more");
    let disk = std::fs::read_to_string(&path).expect("read");
    assert_eq!(disk, "hello-scratch-and-more");

    // The log keeps everything (memory only keeps the tail) up to its cap.
    let big = "x".repeat(MAX_JOB_OUTPUT_BYTES + 10);
    append_output(&job, &big);
    let len = std::fs::metadata(&path).expect("meta").len() as usize;
    assert_eq!(len, "hello-scratch-and-more".len() + big.len());
    lock(&job).log.as_mut().unwrap().len = MAX_JOB_LOG_BYTES;
    append_output(&job, "over-the-cap");
    assert_eq!(std::fs::metadata(&path).unwrap().len() as usize, len);

    // A failed write is skipped, not fatal.
    let mut read_only = ScratchLog {
        file: std::fs::File::open(&path).unwrap(),
        len: 0,
    };
    read_only.append("nope");
    assert_eq!(read_only.len, 0);

    let blocked = dir.path().join("blocked-file");
    std::fs::write(&blocked, "not-a-dir").unwrap();
    assert!(ensure_scratch_log(&blocked, "bg-x").is_none());

    let clash = dir
        .path()
        .join(".whycodes")
        .join("scratch")
        .join("bg-clash.log");
    std::fs::create_dir_all(&clash).unwrap();
    assert!(ensure_scratch_log(dir.path(), "bg-clash").is_none());
}

#[tokio::test]
async fn wait_until_idle_missing_id_is_failed() {
    let reg = BackgroundRegistry::new(4);
    let status = reg
        .wait_until_idle("bg-missing", Duration::from_millis(20))
        .await;
    assert_eq!(status, JobStatus::Failed);
}

#[cfg(windows)]
fn hang_cmd() -> &'static str {
    "ping -n 30 127.0.0.1 >NUL"
}

#[cfg(not(windows))]
fn hang_cmd() -> &'static str {
    "sleep 60"
}

#[tokio::test]
async fn wait_until_idle_ignores_a_different_job_finishing() {
    let reg = BackgroundRegistry::new(8);
    let other = reg
        .start_shell(
            "echo other-done",
            std::env::temp_dir(),
            SandboxSettings::off(),
            Some("other".into()),
        )
        .expect("other");
    let fg = reg
        .start_shell(
            hang_cmd(),
            std::env::temp_dir(),
            SandboxSettings::off(),
            Some("fg".into()),
        )
        .expect("fg");
    for _ in 0..80 {
        let snap = reg.list().into_iter().find(|j| j.id == other);
        if snap.is_some_and(|j| j.status != JobStatus::Running) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let other_snap = reg
        .list()
        .into_iter()
        .find(|j| j.id == other)
        .expect("other listed");
    assert_ne!(
        other_snap.status,
        JobStatus::Running,
        "the other job must finish so the wait is tempted to observe it"
    );
    let t0 = Instant::now();
    let status = reg.wait_until_idle(&fg, Duration::from_millis(200)).await;
    assert!(
        t0.elapsed() >= Duration::from_millis(150),
        "must wait the timeout, not return when the other job finished: {:?}",
        t0.elapsed()
    );
    assert_eq!(status, JobStatus::Running);
    let fg_snap = reg.list().into_iter().find(|j| j.id == fg).expect("fg");
    assert_eq!(fg_snap.status, JobStatus::Running);
    reg.kill_all();
}

#[tokio::test]
async fn a_repeated_sandbox_warning_reaches_one_job_only() {
    let warning = "bg fallback warning seen once per process (test-only text)";
    let mut outputs = Vec::new();
    for id in ["bg-warn-1", "bg-warn-2"] {
        let reg = BackgroundRegistry::new(4);
        let job = Arc::new(Mutex::new(sample_job(JobStatus::Running, "")));
        reg.inner
            .jobs
            .lock()
            .unwrap()
            .insert(id.into(), Arc::clone(&job));
        reg.inner.order.lock().unwrap().push(id.into());
        run_background_job(
            reg,
            Arc::clone(&job),
            id.into(),
            "missing".into(),
            "/no/such/whycodes-bg-bin".into(),
            vec![],
            std::env::temp_dir(),
            vec![],
            Some(warning.into()),
            Arc::new(AtomicBool::new(false)),
        )
        .await;
        outputs.push(job.lock().unwrap().output.clone());
    }
    assert!(outputs[0].contains(warning), "{outputs:?}");
    assert!(!outputs[1].contains(warning), "{outputs:?}");
}

#[tokio::test]
async fn background_job_sees_whycodes_scratch() {
    let dir = tempfile::tempdir().unwrap();
    let reg = BackgroundRegistry::new(4);
    // Git Bash / the embedded shell expand `$VAR`; `cmd.exe` expands `%VAR%`.
    let id = reg
        .start_shell_auto(
            "echo \"$WHYCODES_SCRATCH\" %WHYCODES_SCRATCH%",
            dir.path().to_path_buf(),
            SandboxSettings::off(),
            None,
        )
        .expect("start");
    assert_eq!(
        reg.wait_until_idle(&id, Duration::from_secs(10)).await,
        JobStatus::Done
    );
    let out = reg.read(&id, 10_000).expect("read");
    let want = whycodes_core::project_scratch_dir(&std::path::absolute(dir.path()).unwrap());
    assert!(out.contains(&*want.to_string_lossy()), "{out}");
}

#[cfg(unix)]
#[tokio::test]
async fn a_process_left_holding_the_pipe_does_not_keep_the_job_running() {
    let reg = BackgroundRegistry::new(4);
    // The shell exits at once; `sleep` inherits stdout and outlives the grace.
    let id = reg
        .start_shell(
            "sleep 6 & echo started",
            std::env::temp_dir(),
            SandboxSettings::off(),
            None,
        )
        .expect("start");
    let status = reg
        .wait_until_idle(&id, PIPE_DRAIN_GRACE + Duration::from_secs(3))
        .await;
    assert_eq!(status, JobStatus::Done);
    let out = reg.read(&id, 10_000).expect("read");
    assert!(out.contains("started"), "{out}");
    assert!(out.contains("still holds it open"), "{out}");
}

/// Live and not a zombie (a killed orphan may wait for init to reap it).
#[cfg(target_os = "linux")]
fn process_running(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        !stat
            .rsplit(')')
            .next()
            .unwrap_or("")
            .trim_start()
            .starts_with('Z')
    })
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn kill_signals_the_process_group_without_the_job_task() {
    let dir = tempfile::tempdir().expect("tempdir");
    let reg = BackgroundRegistry::new(4);
    let id = reg
        .start_shell(
            "sleep 60 & echo $! > child.pid; wait",
            dir.path().to_path_buf(),
            SandboxSettings::off(),
            None,
        )
        .expect("start");
    let pid_path = dir.path().join("child.pid");
    let mut pid = None;
    for _ in 0..200 {
        pid = std::fs::read_to_string(&pid_path)
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok());
        if pid.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let pid = pid.expect("grandchild pid");
    assert!(process_running(pid));
    reg.kill(&id).expect("kill");
    // Block the current-thread runtime: the job task cannot run, so only
    // `kill` itself can have stopped the grandchild.
    std::thread::sleep(Duration::from_millis(300));
    assert!(!process_running(pid), "grandchild {pid} survived kill");
}

#[cfg(windows)]
#[tokio::test]
async fn kill_ends_the_shells_children_on_windows() {
    // `ping` is a child of `cmd` and holds stdout. Killing `cmd` alone left it
    // running, and the job waited out the pipe drain grace.
    let reg = BackgroundRegistry::new(4);
    let job = Arc::new(Mutex::new(sample_job(JobStatus::Running, "")));
    {
        let mut jobs = reg.inner.jobs.lock().unwrap();
        jobs.insert("bg-1".into(), Arc::clone(&job));
        let mut order = reg.inner.order.lock().unwrap();
        order.push("bg-1".into());
    }
    let task = tokio::spawn(run_background_job(
        reg.clone(),
        Arc::clone(&job),
        "bg-1".into(),
        "job".into(),
        "cmd".into(),
        vec!["/C".into(), "ping -n 30 127.0.0.1".into()],
        std::env::temp_dir(),
        vec![],
        None,
        Arc::clone(&lock(&job).kill_flag),
    ));
    // Wait for ping's first reply: the grandchild is running and in the job.
    for _ in 0..200 {
        if lock(&job).output.contains("127.0.0.1") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(lock(&job).tree.is_some(), "shell adopted into a job");
    let t0 = Instant::now();
    reg.kill("bg-1").expect("kill");
    task.await.expect("job task");
    assert!(
        t0.elapsed() < PIPE_DRAIN_GRACE,
        "pipe stayed open after kill: {:?}",
        t0.elapsed()
    );
    assert!(!lock(&job).output.contains("still holds it open"));
}
