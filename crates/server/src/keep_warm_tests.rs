use super::*;
use whycodes_core::types::ContentBlock;
use whycodes_llm::{ScriptedProvider, ScriptedStep};
use whycodes_session::session::Session;

use crate::http_tests::{IsolatedHome, test_state_with_registry};

const MS: Duration = Duration::from_millis(1);

fn agent(steps: Vec<ScriptedStep>) -> Arc<Agent> {
    let mut registry = whycodes_llm::provider::ProviderRegistry::new();
    registry.register(Box::new(ScriptedProvider::repeating("script", steps)));
    test_state_with_registry(Some(registry)).agent
}

fn usage_then_text() -> Vec<ScriptedStep> {
    vec![
        ScriptedStep::Usage {
            input_tokens: 3,
            output_tokens: 0,
        },
        ScriptedStep::Text("x".into()),
    ]
}

fn chatted() -> SessionHandle {
    let mut s = Session::new(std::env::temp_dir(), "sys".into());
    s.add_user_message("hello");
    s.add_assistant_message(vec![ContentBlock::Text { text: "hi".into() }]);
    Arc::new(tokio::sync::Mutex::new(s))
}

fn route() -> Route {
    ("script".into(), "m".into(), "k".into())
}

#[tokio::test]
async fn off_when_the_window_is_zero_or_a_turn_runs() {
    assert!(spawn(agent(usage_then_text()), chatted(), route(), 0).is_none());
    let handle = chatted();
    let _turn = handle.lock().await;
    assert!(spawn(agent(usage_then_text()), Arc::clone(&handle), route(), 30).is_none());
}

#[tokio::test]
async fn keeps_warm_until_another_turn_adds_history() {
    let _home = IsolatedHome::new();
    let handle = chatted();
    let task = spawn_every(
        agent(usage_then_text()),
        Arc::clone(&handle),
        route(),
        Duration::from_secs(30),
        5 * MS,
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while handle.lock().await.cost.unpriced_steps == 0 {
        assert!(Instant::now() < deadline, "no keep-alive recorded");
        tokio::time::sleep(5 * MS).await;
    }
    handle.lock().await.add_user_message("next turn");
    assert!(task.await.unwrap() >= 1);
}

#[tokio::test]
async fn stops_when_the_window_is_over_or_nothing_to_keep() {
    let window_over = spawn_every(
        agent(usage_then_text()),
        chatted(),
        route(),
        10 * MS,
        20 * MS,
    );
    assert_eq!(window_over.unwrap().await.unwrap(), 0);

    let empty = Arc::new(tokio::sync::Mutex::new(Session::new(
        std::env::temp_dir(),
        "sys".into(),
    )));
    let task = spawn_every(agent(usage_then_text()), empty, route(), 60 * MS, MS);
    assert_eq!(task.unwrap().await.unwrap(), 0, "no history to keep");

    let failing = agent(vec![ScriptedStep::FailOpen("401 invalid x-api-key".into())]);
    let task = spawn_every(failing, chatted(), route(), 80 * MS, 10 * MS);
    assert_eq!(
        task.unwrap().await.unwrap(),
        0,
        "failed keep-alives are retried, not counted"
    );
}

#[tokio::test]
async fn stops_when_a_turn_holds_or_changes_the_session() {
    let handle = chatted();
    let task = spawn_every(
        agent(usage_then_text()),
        Arc::clone(&handle),
        route(),
        Duration::from_secs(30),
        40 * MS,
    )
    .unwrap();
    tokio::time::sleep(10 * MS).await;
    let busy = handle.lock().await;
    assert_eq!(task.await.unwrap(), 0, "a running turn holds the lock");
    drop(busy);

    let slow = agent(vec![
        ScriptedStep::Hang(Duration::from_millis(150)),
        ScriptedStep::Usage {
            input_tokens: 3,
            output_tokens: 0,
        },
        ScriptedStep::Text("x".into()),
    ]);
    let task = spawn_every(
        slow,
        Arc::clone(&handle),
        route(),
        Duration::from_secs(30),
        5 * MS,
    )
    .unwrap();
    tokio::time::sleep(60 * MS).await;
    handle
        .lock()
        .await
        .add_user_message("turn started meanwhile");
    assert_eq!(
        task.await.unwrap(),
        0,
        "usage of a stale keep-alive is dropped"
    );

    let grown = chatted();
    let task = spawn_every(
        agent(usage_then_text()),
        Arc::clone(&grown),
        route(),
        Duration::from_secs(30),
        40 * MS,
    )
    .unwrap();
    tokio::time::sleep(10 * MS).await;
    grown.lock().await.add_user_message("new turn");
    assert_eq!(task.await.unwrap(), 0, "history grew before the keep-alive");
}
