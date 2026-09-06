#[tokio::test]
async fn dispatch_web_stub() {
    let cli = crate::Cli {
        command: Some(crate::Commands::Web),
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
    super::dispatch_command(&crate::Commands::Web, &cli)
        .await
        .unwrap();
}
