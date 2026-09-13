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

#[tokio::test]
async fn dispatch_slop_json_non_git() {
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("WHYCODES_HOME");
    unsafe { std::env::set_var("WHYCODES_HOME", home.path()) };
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
    };
    let result = super::dispatch_command(cli.command.as_ref().unwrap(), &cli).await;
    unsafe {
        match prev {
            Some(v) => std::env::set_var("WHYCODES_HOME", v),
            None => std::env::remove_var("WHYCODES_HOME"),
        }
    }
    assert!(result.is_err());
}
