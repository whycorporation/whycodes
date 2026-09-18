use super::*;
use crate::cmd::helpers::IsolatedHome;
use crate::{Cli, Commands};
use clap::Parser;

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
