use super::*;

#[test]
fn stale_when_pid_dead() {
    let lock = ServeLock {
        pid: 1,
        port: 3030,
        started_at: now_secs(),
        interactive: false,
        parent_pid: None,
    };
    assert!(is_stale(&lock, now_secs(), |_| PidProbe::Dead));
    assert!(!is_stale(&lock, now_secs(), |_| PidProbe::Alive));
    assert!(!is_stale(&lock, now_secs(), |_| PidProbe::Denied));
}

#[test]
fn stale_when_older_than_max_age() {
    let lock = ServeLock {
        pid: 1,
        port: 3030,
        started_at: 10,
        interactive: false,
        parent_pid: None,
    };
    let now = 10 + MAX_AGE.as_secs() + 1;
    assert!(is_stale(&lock, now, |_| PidProbe::Alive));
}

#[test]
fn stale_when_started_in_the_future() {
    let now = 1_700_000_000;
    let lock = ServeLock {
        pid: 1,
        port: 3030,
        started_at: now + CLOCK_SKEW.as_secs() + 1,
        interactive: false,
        parent_pid: None,
    };
    assert!(is_stale(&lock, now, |_| PidProbe::Alive));
}

#[test]
fn roundtrip_lock_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("serve.lock");
    let lock = ServeLock {
        pid: 42,
        port: 3030,
        started_at: 99,
        interactive: true,
        parent_pid: Some(1),
    };
    write_lock(&path, &lock).unwrap();
    assert_eq!(read_lock(&path), Some(lock));
    remove_lock(&path);
    assert!(read_lock(&path).is_none());
}

#[test]
fn decide_lock_free_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("serve.lock");
    match decide_lock(&path, 3030, true, false).unwrap() {
        LockDecision::Free => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn decide_lock_blocked_without_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("serve.lock");
    let lock = ServeLock {
        pid: std::process::id(),
        port: 3030,
        started_at: now_secs(),
        interactive: false,
        parent_pid: None,
    };
    write_lock(&path, &lock).unwrap();
    match decide_lock(&path, 3030, true, false).unwrap() {
        LockDecision::Blocked(h) => assert_eq!(h.pid, lock.pid),
        other => panic!("{other:?}"),
    }
}

#[test]
fn self_pid_is_alive() {
    assert_eq!(pid_alive(std::process::id()), PidProbe::Alive);
}

#[test]
fn lock_path_is_under_whycodes() {
    let p = lock_path(Path::new("/tmp/proj"));
    assert!(p.ends_with(".whycodes/serve.lock") || p.ends_with(".whycodes\\serve.lock"));
}

#[test]
fn blocked_message_names_pid_and_url() {
    let msg = blocked_message(&ServeLock {
        pid: 9,
        port: 4040,
        started_at: 0,
        interactive: false,
        parent_pid: None,
    });
    assert!(msg.contains("pid 9"), "{msg}");
    assert!(msg.contains("127.0.0.1:4040"), "{msg}");
}

#[test]
fn acquire_lock_free_then_drop_removes_file() {
    let dir = tempfile::tempdir().unwrap();
    let (guard, takeover) = acquire_lock(dir.path(), 3030, true).unwrap();
    assert!(takeover.is_none());
    let path = lock_path(dir.path());
    assert!(path.exists());
    commit_lock(&guard, 4040).unwrap();
    let written = read_lock(&path).unwrap();
    assert_eq!(written.port, 4040);
    drop(guard);
    assert!(read_lock(&path).is_none());
}

#[test]
fn acquire_lock_blocked_when_live_and_no_takeover() {
    let dir = tempfile::tempdir().unwrap();
    let path = lock_path(dir.path());
    write_lock(
        &path,
        &ServeLock {
            pid: std::process::id(),
            port: 3030,
            started_at: now_secs(),
            interactive: false,
            parent_pid: None,
        },
    )
    .unwrap();
    let err = match acquire_lock(dir.path(), 3030, true) {
        Err(e) => e,
        Ok(_) => panic!("expected blocked lock"),
    };
    assert!(err.to_string().contains("already running"), "{err}");
}

#[test]
fn decide_lock_frees_stale_holder() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("serve.lock");
    write_lock(
        &path,
        &ServeLock {
            // PID 1 is init/systemd; kill(1, 0) is often EPERM (Denied = live).
            pid: unused_pid(),
            port: 3030,
            started_at: 10,
            interactive: false,
            parent_pid: None,
        },
    )
    .unwrap();
    match decide_lock(&path, 3030, true, false).unwrap() {
        LockDecision::Free => {}
        other => panic!("{other:?}"),
    }
    assert!(read_lock(&path).is_none());
}

#[test]
fn connect_hint_none_when_stale_or_missing() {
    let dir = tempfile::tempdir().unwrap();
    assert!(connect_hint(dir.path()).is_none());
    let path = lock_path(dir.path());
    write_lock(
        &path,
        &ServeLock {
            pid: std::process::id(),
            port: 9090,
            started_at: now_secs(),
            interactive: false,
            parent_pid: None,
        },
    )
    .unwrap();
    let hint = connect_hint(dir.path()).unwrap();
    assert!(hint.contains("9090"), "{hint}");
    write_lock(
        &path,
        &ServeLock {
            pid: unused_pid(),
            port: 9090,
            started_at: 1,
            interactive: false,
            parent_pid: None,
        },
    )
    .unwrap();
    assert!(connect_hint(dir.path()).is_none());
}

/// Well above typical `pid_max`; `kill` returns ESRCH (Dead), not EPERM.
fn unused_pid() -> u32 {
    2_000_000_000
}

#[test]
fn signal_unknown_pid_errors() {
    // Cover the unix kill wrapper without signalling this process.
    // Never use u32::MAX: `pid as i32` is -1, and kill(-1, SIGTERM)
    // broadcasts to every process we can signal.
    let err = signal_term(unused_pid()).unwrap_err();
    // ESRCH (3). Rust may map it to Uncategorized, not NotFound (ENOENT).
    assert_eq!(err.raw_os_error(), Some(3), "{err:?}");
    let err = signal_kill(unused_pid()).unwrap_err();
    assert_eq!(err.raw_os_error(), Some(3), "{err:?}");
}

#[test]
fn invalid_unix_pids_are_not_broadcast() {
    assert_eq!(pid_alive(0), PidProbe::Dead);
    assert_eq!(pid_alive(u32::MAX), PidProbe::Dead);
    for pid in [0, u32::MAX] {
        let err = signal_term(pid).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound, "{pid}");
        let err = signal_kill(pid).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound, "{pid}");
    }
}

#[test]
fn proc_stat_is_zombie_parses_comm_and_state() {
    assert!(proc_stat_is_zombie("1 (init) Z 0 1 1"));
    assert!(proc_stat_is_zombie("42 (name with ) paren) Z 1 2"));
    assert!(!proc_stat_is_zombie("1 (init) R 0 1 1"));
    assert!(!proc_stat_is_zombie("no-paren-here"));
    #[cfg(target_os = "linux")]
    {
        assert!(!proc_is_zombie(unused_pid() as i32));
        assert!(!proc_is_zombie(std::process::id() as i32));
    }
}

#[test]
fn apply_takeover_choice_covers_all_arms() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("serve.lock");
    let holder = ServeLock {
        pid: 7,
        port: 3030,
        started_at: now_secs(),
        interactive: false,
        parent_pid: None,
    };
    write_lock(&path, &holder).unwrap();
    match apply_takeover_choice(&path, 3030, holder.clone(), Ok(0)).unwrap() {
        LockDecision::Takeover(h) => assert_eq!(h.pid, 7),
        other => panic!("{other:?}"),
    }
    match apply_takeover_choice(&path, 4040, holder.clone(), Ok(2)).unwrap() {
        LockDecision::Free => {}
        other => panic!("{other:?}"),
    }
    assert!(read_lock(&path).is_none());
    write_lock(&path, &holder).unwrap();
    let err = apply_takeover_choice(&path, 3030, holder.clone(), Ok(2)).unwrap_err();
    assert!(err.to_string().contains("still held"), "{err}");
    let err = apply_takeover_choice(&path, 3030, holder.clone(), Ok(1)).unwrap_err();
    assert!(err.to_string().contains("aborted"), "{err}");
    let err = apply_takeover_choice(&path, 3030, holder, Err(())).unwrap_err();
    assert!(err.to_string().contains("aborted"), "{err}");
}

#[test]
fn acquire_lock_takeover_via_test_env() {
    let dir = tempfile::tempdir().unwrap();
    let path = lock_path(dir.path());
    write_lock(
        &path,
        &ServeLock {
            pid: std::process::id(),
            port: 3030,
            started_at: now_secs(),
            interactive: false,
            parent_pid: None,
        },
    )
    .unwrap();
    let prev = std::env::var_os("WHYCODES_TEST_TAKEOVER");
    unsafe { std::env::set_var("WHYCODES_TEST_TAKEOVER", "0") };
    let result = acquire_lock(dir.path(), 3030, false);
    match prev {
        Some(v) => unsafe { std::env::set_var("WHYCODES_TEST_TAKEOVER", v) },
        None => unsafe { std::env::remove_var("WHYCODES_TEST_TAKEOVER") },
    }
    let (guard, takeover) = result.unwrap();
    assert!(takeover.is_some());
    drop(guard);
}

#[test]
fn read_lock_rejects_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("serve.lock");
    std::fs::write(&path, "not-json").unwrap();
    assert!(read_lock(&path).is_none());
    remove_lock(&path);
    remove_lock(&path);
}

#[test]
fn current_parent_pid_is_set_on_unix() {
    let lock = current_lock(9);
    assert_eq!(lock.port, 9);
    #[cfg(unix)]
    assert!(lock.parent_pid.is_some());
}
