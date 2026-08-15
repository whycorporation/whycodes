//! Live protocol v1 against a spawned `whycode serve` (no LLM required).

use std::net::TcpListener;
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::process::Command;
use whycode_sdk::{ErrorCode, LaunchOptions, PermissionDecision, RunOptions, WhycodeClient};

fn ephemeral_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port()
}

async fn wait_connect(base: &str) -> WhycodeClient {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match WhycodeClient::connect(base).await {
            Ok(c) => return c,
            Err(e) if Instant::now() >= deadline => {
                panic!("daemon at {base} never became healthy: {e}");
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
}

#[tokio::test]
async fn v1_session_models_and_errors_without_llm() {
    let home = tempfile::tempdir().expect("home");
    let port = ephemeral_port();
    let bin = env!("CARGO_BIN_EXE_whycode");
    let mut child = Command::new(bin)
        .arg("serve")
        .arg(port.to_string())
        .env("WHYCODE_HOME", home.path())
        .current_dir(home.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn serve");

    let base = format!("127.0.0.1:{port}");
    let client = wait_connect(&base).await;
    let hs = client.health().await.expect("health");
    assert_eq!(hs.protocol, 1);

    let session = client
        .create_session(Some(home.path().display().to_string()))
        .await
        .expect("create");
    assert!(!session.id.is_empty());

    let listed = client.list_sessions().await.expect("list");
    assert!(listed.iter().any(|s| s.id == session.id));

    let hist = client
        .get_history(&session.id, None)
        .await
        .expect("history");
    assert!(hist.messages.is_empty());
    let peek = client.peek(&session.id, 5).await.expect("peek");
    assert!(peek.is_empty());

    let _models = client.list_models().await.expect("models");

    client
        .set_model(&session.id, "openai", "gpt-4o")
        .await
        .expect("set model");

    let renamed = client
        .rename_session(&session.id, "e2e session")
        .await
        .expect("rename");
    assert_eq!(renamed.title, "e2e session");

    client
        .compact(&session.id, Some(150_000))
        .await
        .expect("compact");
    client.rewind(&session.id, 0).await.expect("rewind");

    let perm_err = client
        .respond_to_permission(&session.id, "perm-nope", PermissionDecision::Deny)
        .await
        .expect_err("unknown perm");
    assert_eq!(perm_err.code, ErrorCode::UnknownSession);

    let cancel_err = client.cancel(&session.id).await.expect_err("no run");
    assert_eq!(cancel_err.code, ErrorCode::UnknownSession);

    let run_err = client
        .run(
            &session.id,
            "hello",
            RunOptions {
                auto_approve: Some(true),
                ..Default::default()
            },
        )
        .await
        .expect_err("no api key");
    assert_eq!(run_err.code, ErrorCode::Auth);

    let _ = child.kill().await;
}

#[tokio::test]
async fn launch_isolated_home_does_not_need_user_keys() {
    let bin = env!("CARGO_BIN_EXE_whycode");
    let work = tempfile::tempdir().expect("work");
    let home = tempfile::tempdir().expect("home");
    let client = WhycodeClient::launch(LaunchOptions {
        working_dir: work.path().to_path_buf(),
        binary: Some(bin.into()),
        inherit_logins: false,
        home: Some(home.path().to_path_buf()),
        startup_timeout: Duration::from_secs(20),
        ..Default::default()
    })
    .await
    .expect("launch isolated");

    let hs = client.health().await.expect("health");
    assert_eq!(hs.protocol, 1);
    let session = client
        .create_session(Some(work.path().display().to_string()))
        .await
        .expect("create");
    assert!(
        home.path().join("whycode.db").exists(),
        "isolated WHYCODE_HOME should hold the session db"
    );
    let _ = session;
    client.close().await.expect("close");
}
