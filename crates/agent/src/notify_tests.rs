use super::*;

#[test]
fn spawn_is_noop_when_off() {
    let cfg = NotifyConfig::default();
    spawn_turn_done(&cfg, "t", "b", Some("abc"));
    spawn_need_input_wait(&cfg, "permission", "bash rm");
    let mut on = cfg;
    on.on = vec!["need_input".into(), "turn_done".into()];
    spawn_need_input_wait(&on, "question", "");
    spawn_need_input_wait(&on, "permission", &"x".repeat(400));
    spawn_need_input_wait(&on, "permission", "short");
    spawn_turn_done(&on, "t", "b", None);
    spawn_need_input(&on, "t", "b", Some("sid"));
    let _ = handle_from_config(&on);
}

#[tokio::test]
async fn spawn_fires_when_channel_configured() {
    let on = NotifyConfig {
        on: vec!["need_input".into(), "turn_done".into()],
        discord_webhook: Some("https://example.invalid/webhook".into()),
        ..Default::default()
    };
    spawn_turn_done(&on, "done", "body", Some("abc"));
    spawn_need_input(&on, "need", "body", None);
    spawn_need_input_wait(&on, "permission", "bash rm");
    spawn_need_input_wait(&on, "question", "");
    spawn_need_input_wait(&on, "permission", &"x".repeat(400));
}
