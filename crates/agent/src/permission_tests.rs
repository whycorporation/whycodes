use super::*;

#[tokio::test]
async fn channel_prompter_forwards_reply() {
    let (prompter, mut rx) = ChannelPermissionPrompter::new();
    let ask = tokio::spawn(async move { prompter.ask("bash", "echo hi").await });
    let req = rx.recv().await.expect("permission request");
    assert_eq!(req.tool_name, "bash");
    assert_eq!(req.detail, "echo hi");
    req.reply.send(true).unwrap();
    assert!(ask.await.unwrap());
}

#[tokio::test]
async fn channel_prompter_denies_when_receiver_dropped() {
    let (prompter, rx) = ChannelPermissionPrompter::new();
    drop(rx);
    assert!(!prompter.ask("bash", "x").await);
}

#[tokio::test]
async fn auto_prompters_agree_with_their_names() {
    assert!(AutoApprovePrompter.ask("bash", "x").await);
    assert!(!AutoDenyPrompter.ask("bash", "x").await);
}

#[tokio::test]
async fn default_prompter_respects_env() {
    // Serialize env mutation: these vars are process-global.
    let prev_approve = std::env::var_os("WHYCODES_AUTO_APPROVE");
    let prev_deny = std::env::var_os("WHYCODES_AUTO_DENY");
    let prev_ci = std::env::var_os("CI");

    unsafe { std::env::set_var("WHYCODES_AUTO_APPROVE", "1") };
    unsafe { std::env::remove_var("WHYCODES_AUTO_DENY") };
    unsafe { std::env::remove_var("CI") };
    let p = default_prompter();
    assert!(p.ask("bash", "x").await, "AUTO_APPROVE=1 must allow");

    unsafe { std::env::remove_var("WHYCODES_AUTO_APPROVE") };
    unsafe { std::env::set_var("WHYCODES_AUTO_DENY", "true") };
    unsafe { std::env::remove_var("CI") };
    let p = default_prompter();
    assert!(!p.ask("bash", "x").await, "AUTO_DENY=true must deny");

    // CI (non-interactive) without explicit flags → deny for safety.
    unsafe { std::env::remove_var("WHYCODES_AUTO_APPROVE") };
    unsafe { std::env::remove_var("WHYCODES_AUTO_DENY") };
    unsafe { std::env::set_var("CI", "1") };
    let p = default_prompter();
    assert!(!p.ask("bash", "x").await, "piped stdin must deny");

    // Restore.
    if let Some(v) = prev_approve {
        unsafe { std::env::set_var("WHYCODES_AUTO_APPROVE", v) };
    } else {
        unsafe { std::env::remove_var("WHYCODES_AUTO_APPROVE") };
    }
    if let Some(v) = prev_deny {
        unsafe { std::env::set_var("WHYCODES_AUTO_DENY", v) };
    } else {
        unsafe { std::env::remove_var("WHYCODES_AUTO_DENY") };
    }
    if let Some(v) = prev_ci {
        unsafe { std::env::set_var("CI", v) };
    } else {
        unsafe { std::env::remove_var("CI") };
    }
}

#[test]
fn stdin_prompter_constructs_and_atty_true_without_ci() {
    let _ = StdinPrompter::default();
    let _ = StdinPrompter::default().with_notify(crate::notify::handle_from_config(
        &whycodes_config::NotifyConfig::default(),
    ));
    let prev_ci = std::env::var_os("CI");
    let prev_approve = std::env::var_os("WHYCODES_AUTO_APPROVE");
    let prev_deny = std::env::var_os("WHYCODES_AUTO_DENY");
    unsafe { std::env::remove_var("CI") };
    unsafe { std::env::remove_var("WHYCODES_AUTO_APPROVE") };
    unsafe { std::env::remove_var("WHYCODES_AUTO_DENY") };
    assert!(atty_stderr());
    let _ = default_prompter();
    if let Some(v) = prev_ci {
        unsafe { std::env::set_var("CI", v) };
    } else {
        unsafe { std::env::remove_var("CI") };
    }
    if let Some(v) = prev_approve {
        unsafe { std::env::set_var("WHYCODES_AUTO_APPROVE", v) };
    } else {
        unsafe { std::env::remove_var("WHYCODES_AUTO_APPROVE") };
    }
    if let Some(v) = prev_deny {
        unsafe { std::env::set_var("WHYCODES_AUTO_DENY", v) };
    } else {
        unsafe { std::env::remove_var("WHYCODES_AUTO_DENY") };
    }
}

#[test]
fn permission_line_allows_yes_variants() {
    assert!(permission_line_allows("y"));
    assert!(permission_line_allows("Y"));
    assert!(permission_line_allows(" yes "));
    assert!(permission_line_allows("A"));
    assert!(permission_line_allows("allow"));
    assert!(!permission_line_allows("n"));
    assert!(!permission_line_allows(""));
    assert!(!permission_line_allows("nope"));
}

#[tokio::test]
async fn stdin_prompter_eof_denies() {
    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return;
    }
    let allowed = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        StdinPrompter::default().ask("bash", ""),
    )
    .await
    .expect("stdin ask must not hang on EOF");
    let _ = allowed;
    let allowed = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        StdinPrompter::default().ask("bash", "rm -rf /tmp/x"),
    )
    .await
    .expect("stdin ask with detail must not hang on EOF");
    let _ = allowed;
}

#[tokio::test]
async fn channel_prompter_with_notify_constructs() {
    let (prompter, rx) = ChannelPermissionPrompter::new();
    let _ = prompter.with_notify(crate::notify::handle_from_config(
        &whycodes_config::NotifyConfig::default(),
    ));
    drop(rx);
}

#[tokio::test]
async fn channel_prompter_with_notify_asks() {
    let cfg = whycodes_config::NotifyConfig {
        on: vec!["need_input".into()],
        discord_webhook: Some("https://example.invalid/webhook".into()),
        ..Default::default()
    };
    let (prompter, mut rx) = ChannelPermissionPrompter::new();
    let prompter = prompter.with_notify(crate::notify::handle_from_config(&cfg));
    let ask = tokio::spawn(async move { prompter.ask("bash", "rm -rf /tmp/x").await });
    let req = rx.recv().await.expect("request");
    assert_eq!(req.tool_name, "bash");
    req.reply.send(true).unwrap();
    assert!(ask.await.unwrap());
}
