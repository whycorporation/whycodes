use super::*;

#[tokio::test]
async fn acp_stub_runs() {
    let cli = crate::Cli {
        command: None,
        provider: None,
        model: None,
        agent_flag: None,
        dir: None,
        plain: true,
        continue_session: false,
        resume: None,
        debug: false,
        no_auto_update: true,
        no_memory: true,
    };
    super::cmd_acp(&cli).await.unwrap();
    super::cmd_pr(&cli, Some("t"), Some("dev")).await.unwrap();
}

#[test]
fn github_printer_helpers() {
    let mut saw_acp = false;
    for line in acp_stub_lines() {
        if line.contains("not yet implemented") {
            saw_acp = true;
        }
    }
    assert!(saw_acp);
    let header = pr_create_header_lines("fix", "dev");
    assert!(header[1].contains("fix"));
    assert!(header[2].contains("dev"));
    assert!(pr_created_line().contains("created"));
    assert!(pr_created_short_line().contains("created"));
    let failed = pr_create_failed_lines("t", "main");
    assert!(failed[0].contains("cli.github.com"));
    assert!(failed[1].contains("main"));
    assert!(pr_list_header_line().contains("Listing"));
    assert!(gh_cli_missing_line().contains("cli.github.com"));
    assert!(pr_view_header_line(12).contains("12"));
    assert!(pr_view_failed_line().contains("Could not view"));
    assert!(pr_create_failed_short_line().contains("Could not create"));
    assert!(issue_view_header_line(7).contains("7"));
    assert!(issue_list_header_line().contains("issues"));
    assert!(!gh_status_ok(Err(std::io::Error::other("missing"))));
    let (_dir, stub) = gh_stub(1);
    assert!(!gh_status_ok(std::process::Command::new(&stub).status()));
}

fn test_cli() -> crate::Cli {
    crate::Cli {
        command: None,
        provider: None,
        model: None,
        agent_flag: None,
        dir: None,
        plain: true,
        continue_session: false,
        resume: None,
        debug: false,
        no_auto_update: true,
        no_memory: true,
    }
}

/// Script that exits with `code`. Windows needs a `.cmd` so `Command` finds it.
fn gh_stub(code: u8) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("stub dir");
    if cfg!(windows) {
        let path = dir.path().join("gh.cmd");
        std::fs::write(&path, format!("@echo off\r\nexit /b {code}\r\n")).unwrap();
        (dir, path.display().to_string())
    } else {
        let path = dir.path().join("gh");
        std::fs::write(&path, format!("#!/bin/sh\nexit {code}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        (dir, path.display().to_string())
    }
}

#[tokio::test]
async fn github_success_paths_use_stub() {
    let cli = test_cli();
    let (_dir, stub) = gh_stub(0);
    let _guard = super::TestGhGuard::set(Some(stub));

    super::cmd_pr(&cli, None, None).await.unwrap();
    super::cmd_pr(&cli, Some("feat"), Some("dev"))
        .await
        .unwrap();
    super::cmd_github(
        &cli,
        &crate::GithubCmd::Pr {
            action: Some(crate::PrAction::List),
        },
    )
    .await
    .unwrap();
    super::cmd_github(&cli, &crate::GithubCmd::Pr { action: None })
        .await
        .unwrap();
    super::cmd_github(
        &cli,
        &crate::GithubCmd::Pr {
            action: Some(crate::PrAction::View { number: 4 }),
        },
    )
    .await
    .unwrap();
    super::cmd_github(
        &cli,
        &crate::GithubCmd::Pr {
            action: Some(crate::PrAction::Create {
                title: None,
                base: None,
            }),
        },
    )
    .await
    .unwrap();
    super::cmd_github(
        &cli,
        &crate::GithubCmd::Pr {
            action: Some(crate::PrAction::Create {
                title: Some("t".into()),
                base: Some("dev".into()),
            }),
        },
    )
    .await
    .unwrap();
    super::cmd_github(&cli, &crate::GithubCmd::Issue { number: None })
        .await
        .unwrap();
    super::cmd_github(&cli, &crate::GithubCmd::Issue { number: Some(9) })
        .await
        .unwrap();

    let _cleared = super::TestGhGuard::set(None);
    assert_eq!(super::gh_program(), "gh");
}

#[tokio::test]
async fn github_spawn_failure_warns() {
    let cli = test_cli();
    let dir = tempfile::tempdir().expect("missing parent");
    let missing = dir.path().join("no-such-gh-binary");
    let _guard = super::TestGhGuard::set(Some(missing.display().to_string()));

    super::cmd_pr(&cli, Some("t"), Some("main")).await.unwrap();
    super::cmd_github(
        &cli,
        &crate::GithubCmd::Pr {
            action: Some(crate::PrAction::List),
        },
    )
    .await
    .unwrap();
    super::cmd_github(
        &cli,
        &crate::GithubCmd::Pr {
            action: Some(crate::PrAction::View { number: 1 }),
        },
    )
    .await
    .unwrap();
    super::cmd_github(
        &cli,
        &crate::GithubCmd::Pr {
            action: Some(crate::PrAction::Create {
                title: Some("x".into()),
                base: Some("main".into()),
            }),
        },
    )
    .await
    .unwrap();
    super::cmd_github(&cli, &crate::GithubCmd::Issue { number: Some(2) })
        .await
        .unwrap();
    super::cmd_github(&cli, &crate::GithubCmd::Issue { number: None })
        .await
        .unwrap();
}

/// `gh` starts and exits non-zero. Distinct from a missing binary: `Command`
/// returns `Ok(status)` so the issue view/list warn arms run.
#[tokio::test]
async fn github_nonzero_exit_warns() {
    let cli = test_cli();
    let (_dir, stub) = gh_stub(1);
    let _guard = super::TestGhGuard::set(Some(stub));

    super::cmd_pr(&cli, Some("t"), Some("main")).await.unwrap();
    super::cmd_github(
        &cli,
        &crate::GithubCmd::Pr {
            action: Some(crate::PrAction::List),
        },
    )
    .await
    .unwrap();
    super::cmd_github(&cli, &crate::GithubCmd::Pr { action: None })
        .await
        .unwrap();
    super::cmd_github(
        &cli,
        &crate::GithubCmd::Pr {
            action: Some(crate::PrAction::View { number: 3 }),
        },
    )
    .await
    .unwrap();
    super::cmd_github(
        &cli,
        &crate::GithubCmd::Pr {
            action: Some(crate::PrAction::Create {
                title: None,
                base: None,
            }),
        },
    )
    .await
    .unwrap();
    super::cmd_github(&cli, &crate::GithubCmd::Issue { number: Some(8) })
        .await
        .unwrap();
    super::cmd_github(&cli, &crate::GithubCmd::Issue { number: None })
        .await
        .unwrap();
}
