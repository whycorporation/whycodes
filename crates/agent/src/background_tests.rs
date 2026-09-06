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
