use super::*;
use std::time::{Duration, SystemTime};
use whycodes_core::{SandboxFallback, SandboxMode, SandboxSettings};

const HOUR: Duration = Duration::from_secs(60 * 60);
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

fn write_aged(path: &Path, bytes: usize, mtime: SystemTime) {
    std::fs::write(path, vec![b'x'; bytes]).unwrap();
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
}

#[cfg(unix)]
fn age_dir(path: &Path, mtime: SystemTime) {
    std::fs::File::open(path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
}

#[cfg(windows)]
fn age_dir(path: &Path, mtime: SystemTime) {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_WRITE_ATTRIBUTES: u32 = 0x100;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    std::fs::File::options()
        .access_mode(FILE_WRITE_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn scratch_env_names_and_creates_the_absolute_scratch_dir() {
    let dir = tempfile::tempdir().unwrap();
    let env = scratch_env(dir.path());
    let want = whycodes_core::project_scratch_dir(&std::path::absolute(dir.path()).unwrap());
    assert_eq!(
        env,
        vec![(SCRATCH_ENV.to_string(), want.clone().into_os_string())]
    );
    assert!(want.is_dir(), "{}", want.display());
    // Second sighting: already pruned this process, no new thread.
    assert_eq!(scratch_env(dir.path()), env);
}

#[test]
fn scratch_env_without_a_working_dir_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("gone");
    let env = scratch_env(&missing);
    assert_eq!(
        PathBuf::from(&env[0].1),
        whycodes_core::project_scratch_dir(&missing)
    );
    assert!(!missing.exists());
    // An empty path cannot be made absolute: it stays relative.
    let env = scratch_env(Path::new(""));
    assert_eq!(PathBuf::from(&env[0].1), PathBuf::from(".whycodes/scratch"));
}

#[test]
fn scratch_env_survives_an_uncreatable_dir() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".whycodes"), "a file, not a dir").unwrap();
    let env = scratch_env(dir.path());
    assert_eq!(env[0].0, SCRATCH_ENV);
    assert!(!PathBuf::from(&env[0].1).exists());
}

#[test]
fn shell_command_sees_whycodes_scratch() {
    let dir = tempfile::tempdir().unwrap();
    let request = crate::SandboxRequest {
        // Git Bash / embedded shell expand `$VAR`; `cmd.exe` expands `%VAR%`.
        command: "echo \"$WHYCODES_SCRATCH\" %WHYCODES_SCRATCH%".into(),
        working_dir: dir.path().to_path_buf(),
        settings: SandboxSettings {
            mode: SandboxMode::Off,
            network: true,
            fallback: SandboxFallback::Allow,
        },
    };
    let out = crate::run(&request).expect("run");
    let want = whycodes_core::project_scratch_dir(&std::path::absolute(dir.path()).unwrap());
    assert!(
        out.stdout_lossy().contains(&*want.to_string_lossy()),
        "stdout={} stderr={}",
        out.stdout_lossy(),
        out.stderr_lossy()
    );
    assert!(want.is_dir());
}

#[test]
fn prune_removes_entries_past_the_age_limit() {
    let dir = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    let old = now - 8 * DAY;
    write_aged(&dir.path().join("old.txt"), 4, old);
    write_aged(&dir.path().join("new.txt"), 4, now);
    let old_dir = dir.path().join("old-dir");
    std::fs::create_dir(&old_dir).unwrap();
    write_aged(&old_dir.join("f"), 4, old);
    age_dir(&old_dir, old);
    // An old directory with one fresh file inside is still in use.
    let mixed = dir.path().join("mixed-dir");
    std::fs::create_dir_all(mixed.join("sub")).unwrap();
    write_aged(&mixed.join("stale"), 4, old);
    write_aged(&mixed.join("sub").join("fresh"), 4, now);
    age_dir(&mixed.join("sub"), old);
    age_dir(&mixed, old);

    let removed = prune_scratch_dir(dir.path(), now, SCRATCH_MAX_AGE, SCRATCH_MAX_BYTES);
    assert_eq!(removed, 2);
    assert_eq!(names(dir.path()), ["mixed-dir", "new.txt"]);
    assert!(mixed.join("stale").exists());
}

#[test]
fn prune_drops_oldest_entries_until_under_the_size_cap() {
    let dir = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    write_aged(&dir.path().join("a.bin"), 100, now - 3 * HOUR);
    write_aged(&dir.path().join("b.bin"), 100, now - 2 * HOUR);
    write_aged(&dir.path().join("c.bin"), 100, now - HOUR);
    // Newest entry; its bytes count recursively (120).
    let d = dir.path().join("d");
    std::fs::create_dir(&d).unwrap();
    write_aged(&d.join("x"), 60, now - 4 * HOUR);
    write_aged(&d.join("y"), 60, now - 4 * HOUR);

    let removed = prune_scratch_dir(dir.path(), now, SCRATCH_MAX_AGE, 150);
    assert_eq!(removed, 3);
    assert_eq!(names(dir.path()), ["d"]);
    assert_eq!(names(&d), ["x", "y"]);

    // Already under the cap: nothing more goes.
    assert_eq!(prune_scratch_dir(dir.path(), now, SCRATCH_MAX_AGE, 150), 0);
}

#[cfg(unix)]
fn link_dir(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

#[cfg(windows)]
fn link_dir(target: &Path, link: &Path) -> bool {
    // Needs Developer Mode or admin; without it the test has nothing to check.
    std::os::windows::fs::symlink_dir(target, link).is_ok()
}

#[test]
fn prune_never_follows_a_symlink_out_of_scratch() {
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("big.bin");
    write_aged(&target, 4096, SystemTime::now() - 30 * DAY);
    let dir = tempfile::tempdir().unwrap();
    write_aged(&dir.path().join("keep.txt"), 10, SystemTime::now());
    if !link_dir(outside.path(), &dir.path().join("link")) {
        return;
    }
    let now = SystemTime::now();
    // The target's 4096 bytes and 30-day age are not the link's.
    assert_eq!(prune_scratch_dir(dir.path(), now, SCRATCH_MAX_AGE, 100), 0);
    assert_eq!(names(dir.path()), ["keep.txt", "link"]);
    // Removing everything removes the link itself, not what it points at.
    assert_eq!(
        prune_scratch_dir(dir.path(), now + HOUR, Duration::ZERO, 0),
        2
    );
    assert!(names(dir.path()).is_empty());
    assert!(target.is_file());
}

#[test]
fn prune_of_a_missing_dir_and_a_vanished_entry_is_a_no_op() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(prune_now(&dir.path().join("missing")), 0);
    assert_eq!(prune_now(dir.path()), 0);
    let gone = Entry {
        path: dir.path().join("vanished"),
        is_file: true,
        newest: SystemTime::UNIX_EPOCH,
        bytes: 1,
    };
    assert!(!remove_entry(&gone));
    // A huge age limit never underflows the clock.
    assert_eq!(
        prune_scratch_dir(dir.path(), SystemTime::UNIX_EPOCH, SCRATCH_MAX_AGE, 0),
        0
    );
}

#[test]
fn prune_once_recovers_from_a_poisoned_lock() {
    let poisoner = std::thread::spawn(|| {
        let _guard = PRUNED.lock().unwrap();
        panic!("poison");
    });
    assert!(poisoner.join().is_err());
    let dir = tempfile::tempdir().unwrap();
    prune_once(dir.path());
    assert!(
        PRUNED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(dir.path())
    );
}
