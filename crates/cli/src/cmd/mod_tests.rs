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
        approve_tools: false,
    };
    super::dispatch_command(&crate::Commands::Web, &cli)
        .await
        .unwrap();
}

#[tokio::test]
async fn dispatch_slop_json_non_git() {
    let dir = tempfile::tempdir().unwrap();
    let _home = super::helpers::IsolatedHome::new();
    let cli = crate::Cli {
        command: Some(crate::Commands::Slop {
            base: None,
            json: true,
        }),
        provider: None,
        model: None,
        agent_flag: None,
        dir: Some(dir.path().display().to_string()),
        plain: true,
        continue_session: false,
        resume: None,
        debug: false,
        no_auto_update: true,
        no_memory: true,
        approve_tools: false,
    };
    let result = super::dispatch_command(cli.command.as_ref().unwrap(), &cli).await;
    assert!(result.is_err());
}
