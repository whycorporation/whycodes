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
    assert!(
        acp_stub_lines()
            .iter()
            .any(|l| l.contains("not yet implemented"))
    );
    let header = pr_create_header_lines("fix", "dev");
    assert!(header.iter().any(|l| l.contains("fix")));
    assert!(header.iter().any(|l| l.contains("dev")));
    assert!(pr_created_line().contains("created"));
    assert!(pr_created_short_line().contains("created"));
    let failed = pr_create_failed_lines("t", "main");
    assert!(failed.iter().any(|l| l.contains("cli.github.com")));
    assert!(pr_list_header_line().contains("Listing"));
    assert!(gh_cli_missing_line().contains("cli.github.com"));
    assert!(pr_view_header_line(12).contains("12"));
    assert!(pr_view_failed_line().contains("Could not view"));
    assert!(pr_create_failed_short_line().contains("Could not create"));
    assert!(issue_view_header_line(7).contains("7"));
    assert!(issue_list_header_line().contains("issues"));
    assert!(!gh_status_ok(Err(std::io::Error::other("missing"))));
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

/// Script that exits 0. Windows needs a `.cmd` so `Command` finds it.
fn ok_gh_stub() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("stub dir");
    if cfg!(windows) {
        let path = dir.path().join("gh.cmd");
        std::fs::write(&path, "@echo off\r\nexit /b 0\r\n").unwrap();
        (dir, path.display().to_string())
    } else {
        let path = dir.path().join("gh");
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
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
    let (_dir, stub) = ok_gh_stub();
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
