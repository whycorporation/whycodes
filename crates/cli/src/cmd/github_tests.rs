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
