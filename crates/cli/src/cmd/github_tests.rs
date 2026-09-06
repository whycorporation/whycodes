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
