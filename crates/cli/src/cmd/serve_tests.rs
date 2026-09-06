#[tokio::test]
async fn web_stub_runs() {
    super::cmd_web().await.unwrap();
}
