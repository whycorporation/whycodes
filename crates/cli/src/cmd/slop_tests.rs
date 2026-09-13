use super::*;
use crate::{Cli, Commands};
use clap::Parser;

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct IsolatedHome {
    _guard: std::sync::MutexGuard<'static, ()>,
    _dir: tempfile::TempDir,
    prev: Option<std::ffi::OsString>,
}

impl IsolatedHome {
    fn new() -> Self {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("WHYCODES_HOME");
        unsafe { std::env::set_var("WHYCODES_HOME", dir.path()) };
        Self {
            _guard: guard,
            _dir: dir,
            prev,
        }
    }
}

impl Drop for IsolatedHome {
    fn drop(&mut self) {
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var("WHYCODES_HOME", v),
                None => std::env::remove_var("WHYCODES_HOME"),
            }
        }
    }
}

#[tokio::test]
async fn slop_json_on_temp_non_git() {
    let _home = IsolatedHome::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().display().to_string();
    let cli = Cli::try_parse_from(["whycodes", "slop", "--json", "-d", &path]).unwrap();
    match &cli.command {
        Some(Commands::Slop { base, json }) => {
            assert!(json);
            assert!(base.is_none());
            assert!(cmd_slop(&cli, base.as_deref(), *json).await.is_err());
        }
        other => panic!("parsed {other:?}"),
    }
}

#[tokio::test]
async fn slop_text_on_git_tree() {
    let _home = IsolatedHome::new();
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .unwrap()
    };
    assert!(git(&["init", "-b", "main"]).success());
    let _ = git(&["config", "user.email", "t@t"]);
    let _ = git(&["config", "user.name", "t"]);
    std::fs::write(dir.path().join("a.rs"), "fn a() { 1 }\n").unwrap();
    assert!(git(&["add", "."]).success());
    assert!(git(&["commit", "-m", "init"]).success());
    std::fs::write(
        dir.path().join("a.rs"),
        "fn a() { if true { 1 } else { 0 } }\n",
    )
    .unwrap();
    let path = dir.path().display().to_string();
    let cli = Cli::try_parse_from(["whycodes", "slop", "--base", "HEAD", "-d", &path]).unwrap();
    match &cli.command {
        Some(Commands::Slop { base, json }) => {
            assert!(!*json);
            cmd_slop(&cli, base.as_deref(), *json).await.unwrap();
        }
        other => panic!("{other:?}"),
    }
    let cli =
        Cli::try_parse_from(["whycodes", "slop", "--json", "--base", "HEAD", "-d", &path]).unwrap();
    match &cli.command {
        Some(Commands::Slop { base, json }) => {
            cmd_slop(&cli, base.as_deref(), *json).await.unwrap();
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn slop_text_error_on_non_git() {
    let _home = IsolatedHome::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().display().to_string();
    let cli = Cli::try_parse_from(["whycodes", "slop", "-d", &path]).unwrap();
    match &cli.command {
        Some(Commands::Slop { base, json }) => {
            assert!(cmd_slop(&cli, base.as_deref(), *json).await.is_err());
        }
        other => panic!("{other:?}"),
    }
}
