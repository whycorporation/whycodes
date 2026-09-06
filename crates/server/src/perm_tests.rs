use super::*;
use whycodes_agent::events::TurnEvent;
use whycodes_tools::question::{QuestionAnswer, QuestionOption, QuestionSpec};

fn scope(session_id: &str, auto_approve: bool, hub: Arc<PermHub>) -> RunScope {
    RunScope {
        session_id: session_id.into(),
        auto_approve,
        hub,
    }
}

#[tokio::test]
async fn permission_ask_outside_a_run_scope_is_refused() {
    let hub = PermHub::new();
    let prompter = ServePrompter { hub };
    assert!(!prompter.ask("bash", "do a thing").await);
}

#[tokio::test]
async fn permission_ask_with_auto_approve_returns_true_immediately() {
    let hub = PermHub::new();
    let prompter = ServePrompter {
        hub: Arc::clone(&hub),
    };
    let s = scope("s1", true, hub);
    let ok = RUN
        .scope(s, async { prompter.ask("bash", "x").await })
        .await;
    assert!(ok);
}

#[tokio::test]
async fn allow_decision_unblocks_the_ask() {
    let hub = PermHub::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    hub.register_run("s1", tx);
    let prompter = ServePrompter {
        hub: Arc::clone(&hub),
    };
    let s = scope("s1", false, Arc::clone(&hub));
    let task = tokio::spawn(async move {
        RUN.scope(s, async { prompter.ask("bash", "run ls").await })
            .await
    });
    let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out waiting for permission ask")
        .expect("event channel closed");
    let TurnEvent::PermissionAsk {
        request_id,
        tool_name,
        detail,
    } = ev
    else {
        panic!("expected PermissionAsk, got {ev:?}");
    };
    assert_eq!(tool_name, "bash");
    assert_eq!(detail, "run ls");
    hub.decide("s1", &request_id, PermissionDecision::Allow)
        .unwrap();
    assert!(task.await.expect("ask task panicked"));
}

#[tokio::test]
async fn deny_decision_unblocks_the_ask_with_false() {
    let hub = PermHub::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    hub.register_run("s1", tx);
    let prompter = ServePrompter {
        hub: Arc::clone(&hub),
    };
    let s = scope("s1", false, Arc::clone(&hub));
    let task = tokio::spawn(async move {
        RUN.scope(s, async { prompter.ask("bash", "x").await })
            .await
    });
    let ev = rx.recv().await.expect("no permission ask");
    let TurnEvent::PermissionAsk { request_id, .. } = ev else {
        panic!("expected PermissionAsk");
    };
    hub.decide("s1", &request_id, PermissionDecision::Deny)
        .unwrap();
    assert!(!task.await.expect("ask task panicked"));
}

#[tokio::test]
async fn allow_always_skips_future_asks() {
    let hub = PermHub::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    hub.register_run("s1", tx);
    let prompter = ServePrompter {
        hub: Arc::clone(&hub),
    };
    let s = scope("s1", false, Arc::clone(&hub));
    let prompter_for_task = prompter.clone();
    let task = tokio::spawn(async move {
        RUN.scope(s, async { prompter_for_task.ask("bash", "first").await })
            .await
    });
    let ev = rx.recv().await.expect("no permission ask");
    let TurnEvent::PermissionAsk { request_id, .. } = ev else {
        panic!("expected PermissionAsk");
    };
    hub.decide("s1", &request_id, PermissionDecision::AllowAlways)
        .unwrap();
    assert!(task.await.expect("ask task panicked"));

    // Second ask for the same tool auto-approves without a new event.
    let s2 = scope("s1", false, Arc::clone(&hub));
    let ok = RUN
        .scope(s2, async { prompter.ask("bash", "second").await })
        .await;
    assert!(ok);
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn deciding_an_unknown_request_is_an_error() {
    let hub = PermHub::new();
    let err = hub
        .decide("s1", "perm-999", PermissionDecision::Allow)
        .unwrap_err();
    assert!(err.contains("unknown permission request"));
}

#[tokio::test]
async fn finish_run_resolves_pending_asks_as_denied() {
    let hub = PermHub::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    hub.register_run("s1", tx);
    let prompter = ServePrompter {
        hub: Arc::clone(&hub),
    };
    let s = scope("s1", false, Arc::clone(&hub));
    let task = tokio::spawn(async move {
        RUN.scope(s, async { prompter.ask("bash", "x").await })
            .await
    });
    let _ = rx.recv().await.expect("no permission ask");
    hub.finish_run("s1");
    assert!(!task.await.expect("ask task panicked"));
    // A second finish for the same session is a no-op.
    hub.finish_run("s1");
}

#[tokio::test]
async fn answer_question_resolves_the_ask() {
    let hub = PermHub::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    hub.register_run("s1", tx);
    let prompter = ServeQuestionPrompter::new(Arc::clone(&hub));
    let s = scope("s1", false, Arc::clone(&hub));
    let questions = vec![QuestionSpec {
        prompt: "Pick a color".into(),
        options: vec![QuestionOption {
            label: "red".into(),
            description: "the reddest".into(),
            preview: Some("#f00".into()),
        }],
        multi_select: false,
        important: false,
    }];
    let task =
        tokio::spawn(async move { RUN.scope(s, async { prompter.ask(questions).await }).await });
    let ev = rx.recv().await.expect("no question ask");
    let TurnEvent::QuestionAsk {
        request_id,
        questions,
    } = ev
    else {
        panic!("expected QuestionAsk");
    };
    assert_eq!(questions[0]["prompt"], "Pick a color");
    assert_eq!(questions[0]["options"][0]["label"], "red");
    assert_eq!(questions[0]["options"][0]["preview"], "#f00");
    hub.answer_question(
        "s1",
        &request_id,
        Some(vec![QuestionAnswerWire {
            selected: vec!["red".into()],
            free_text: None,
        }]),
        false,
    )
    .unwrap();
    let answers = task.await.expect("ask task panicked").unwrap();
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0].selected, vec!["red".to_string()]);
}

#[tokio::test]
async fn cancelled_answer_maps_to_a_cancelled_error() {
    let hub = PermHub::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    hub.register_run("s1", tx);
    let prompter = ServeQuestionPrompter::new(Arc::clone(&hub));
    let s = scope("s1", false, Arc::clone(&hub));
    let task =
        tokio::spawn(async move { RUN.scope(s, async { prompter.ask(Vec::new()).await }).await });
    let ev = rx.recv().await.expect("no question ask");
    let TurnEvent::QuestionAsk { request_id, .. } = ev else {
        panic!("expected QuestionAsk");
    };
    hub.answer_question("s1", &request_id, None, true).unwrap();
    assert_eq!(
        task.await.expect("ask task panicked").unwrap_err(),
        QuestionError::Cancelled
    );
}

#[tokio::test]
async fn answering_an_unknown_question_is_an_error() {
    let hub = PermHub::new();
    let err = hub.answer_question("s1", "q-999", None, false).unwrap_err();
    assert!(err.contains("unknown question request"));
    let err = hub
        .answer_question("s1", "q-missing", None, true)
        .unwrap_err();
    assert!(err.contains("unknown question request"));
}

#[tokio::test]
async fn answer_question_rejects_unknown_labels() {
    let hub = PermHub::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    hub.register_run("s1", tx);
    let prompter = ServeQuestionPrompter::new(Arc::clone(&hub));
    let s = scope("s1", false, Arc::clone(&hub));
    let questions = vec![QuestionSpec {
        prompt: "Pick".into(),
        options: vec![QuestionOption {
            label: "A".into(),
            description: String::new(),
            preview: None,
        }],
        multi_select: false,
        important: false,
    }];
    let task =
        tokio::spawn(async move { RUN.scope(s, async { prompter.ask(questions).await }).await });
    let ev = rx.recv().await.expect("no question ask");
    let TurnEvent::QuestionAsk { request_id, .. } = ev else {
        panic!("expected QuestionAsk");
    };
    let err = hub
        .answer_question(
            "s1",
            &request_id,
            Some(vec![QuestionAnswerWire {
                selected: vec!["nope".into()],
                free_text: None,
            }]),
            false,
        )
        .unwrap_err();
    assert!(err.contains("unknown option"), "{err}");
    hub.answer_question("s1", &request_id, None, true).unwrap();
    assert_eq!(
        task.await.expect("ask task panicked").unwrap_err(),
        QuestionError::Cancelled
    );
}

#[tokio::test]
async fn question_ask_with_auto_approve_short_circuits() {
    let hub = PermHub::new();
    let prompter = ServeQuestionPrompter::new(Arc::clone(&hub));
    let s = scope("s1", true, Arc::clone(&hub));
    let questions = vec![QuestionSpec {
        prompt: "ok?".into(),
        options: vec![QuestionOption {
            label: "yes".into(),
            description: String::new(),
            preview: None,
        }],
        multi_select: false,
        important: false,
    }];
    let answers = RUN
        .scope(s, async { prompter.ask(questions).await })
        .await
        .unwrap();
    assert_eq!(answers.len(), 1);
    // AutoAnswerPrompter selects the first option by default.
    assert_eq!(
        answers[0],
        QuestionAnswer {
            selected: vec!["yes".to_string()],
            free_text: None,
            auto_picked: true,
        }
    );
}

#[tokio::test]
async fn question_ask_outside_a_run_scope_is_disconnected() {
    let hub = PermHub::new();
    let prompter = ServeQuestionPrompter::new(hub);
    assert_eq!(
        prompter.ask(Vec::new()).await.unwrap_err(),
        QuestionError::Disconnected
    );
}

fn poison_mutex<T>(m: &Mutex<T>) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _g = m.lock().unwrap();
        panic!("poison");
    }));
}

#[tokio::test]
async fn finish_run_resolves_pending_questions() {
    let hub = PermHub::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    hub.register_run("s1", tx);
    let prompter = ServeQuestionPrompter::new(Arc::clone(&hub));
    let s = scope("s1", false, Arc::clone(&hub));
    let task =
        tokio::spawn(async move { RUN.scope(s, async { prompter.ask(Vec::new()).await }).await });
    let _ = rx.recv().await.expect("no question ask");
    hub.finish_run("s1");
    assert_eq!(
        task.await.expect("ask task panicked").unwrap_err(),
        QuestionError::Disconnected
    );
}

#[tokio::test]
async fn dropped_reply_denies_permission_and_disconnects_question() {
    let hub = PermHub::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    hub.register_run("s1", tx);
    let prompter = ServePrompter {
        hub: Arc::clone(&hub),
    };
    let s = scope("s1", false, Arc::clone(&hub));
    let task = tokio::spawn(async move {
        RUN.scope(s, async { prompter.ask("bash", "x").await })
            .await
    });
    let _ = rx.recv().await.expect("no permission ask");
    let pending = {
        let mut map = hub.pending.lock().unwrap();
        let key = map.keys().next().cloned().expect("pending key");
        map.remove(&key)
    };
    drop(pending);
    assert!(!task.await.expect("ask task panicked"));

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    hub.register_run("s1", tx);
    let q = ServeQuestionPrompter::new(Arc::clone(&hub));
    let s = scope("s1", false, Arc::clone(&hub));
    let task = tokio::spawn(async move { RUN.scope(s, async { q.ask(Vec::new()).await }).await });
    let _ = rx.recv().await.expect("no question ask");
    let pending = {
        let mut map = hub.pending_q.lock().unwrap();
        let key = map.keys().next().cloned().expect("pending key");
        map.remove(&key)
    };
    drop(pending);
    assert_eq!(
        task.await.expect("ask task panicked").unwrap_err(),
        QuestionError::Disconnected
    );
}

#[test]
fn decide_and_answer_fail_after_the_asker_is_gone() {
    let hub = PermHub::new();
    let (tx, rx) = oneshot::channel();
    drop(rx);
    hub.pending.lock().unwrap().insert(
        ("s1".into(), "perm-1".into()),
        Pending {
            tool_name: "bash".into(),
            reply: tx,
        },
    );
    let err = hub
        .decide("s1", "perm-1", PermissionDecision::Allow)
        .unwrap_err();
    assert!(err.contains("timed out"), "{err}");

    let (tx, rx) = oneshot::channel::<Result<Vec<QuestionAnswer>, QuestionError>>();
    drop(rx);
    hub.pending_q.lock().unwrap().insert(
        ("s1".into(), "q-1".into()),
        PendingQuestion {
            questions: Vec::new(),
            reply: tx,
        },
    );
    let err = hub.answer_question("s1", "q-1", None, false).unwrap_err();
    assert!(err.contains("timed out"), "{err}");

    let (tx, rx) = oneshot::channel::<Result<Vec<QuestionAnswer>, QuestionError>>();
    drop(rx);
    hub.pending_q.lock().unwrap().insert(
        ("s1".into(), "q-cancel".into()),
        PendingQuestion {
            questions: Vec::new(),
            reply: tx,
        },
    );
    let err = hub
        .answer_question("s1", "q-cancel", None, true)
        .unwrap_err();
    assert!(err.contains("timed out"), "{err}");
}

#[test]
fn emit_ignores_closed_and_poisoned_event_maps() {
    let hub = PermHub::new();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    hub.register_run("s1", tx);
    drop(rx);
    hub.emit("s1", TurnEvent::Cancelled);

    poison_mutex(&hub.events);
    hub.register_run("s2", tokio::sync::mpsc::unbounded_channel().0);
    hub.finish_run("s2");
    hub.emit("s2", TurnEvent::Cancelled);
}

#[test]
fn poisoned_maps_are_safe_noops_or_errors() {
    let hub = PermHub::new();
    poison_mutex(&hub.pending);
    poison_mutex(&hub.pending_q);
    poison_mutex(&hub.allow_always);
    hub.finish_run("s1");
    assert!(!hub.is_always_allowed("s1", "bash"));
    assert!(
        hub.decide("s1", "x", PermissionDecision::Allow)
            .unwrap_err()
            .contains("poisoned")
    );
    assert!(
        hub.answer_question("s1", "x", None, false)
            .unwrap_err()
            .contains("poisoned")
    );
}

#[tokio::test]
async fn poisoned_pending_refuses_new_asks() {
    let hub = PermHub::new();
    poison_mutex(&hub.pending);
    poison_mutex(&hub.pending_q);
    let p = ServePrompter {
        hub: Arc::clone(&hub),
    };
    let q = ServeQuestionPrompter::new(Arc::clone(&hub));
    let s = scope("s1", false, Arc::clone(&hub));
    assert!(
        !RUN.scope(s.clone(), async { p.ask("bash", "x").await })
            .await
    );
    assert_eq!(
        RUN.scope(s, async { q.ask(Vec::new()).await })
            .await
            .unwrap_err(),
        QuestionError::Disconnected
    );
}

#[tokio::test]
async fn allow_always_survives_poisoned_set_and_still_allows() {
    let hub = PermHub::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    hub.register_run("s1", tx);
    poison_mutex(&hub.allow_always);
    let prompter = ServePrompter {
        hub: Arc::clone(&hub),
    };
    let s = scope("s1", false, Arc::clone(&hub));
    let task = tokio::spawn(async move {
        RUN.scope(s, async { prompter.ask("bash", "x").await })
            .await
    });
    let ev = rx.recv().await.expect("no permission ask");
    let TurnEvent::PermissionAsk { request_id, .. } = ev else {
        panic!("expected PermissionAsk");
    };
    hub.decide("s1", &request_id, PermissionDecision::AllowAlways)
        .unwrap();
    assert!(task.await.expect("ask task panicked"));
}

#[test]
fn leftover_send_is_ignored_when_asker_already_returned() {
    let hub = PermHub::new();
    let (tx, rx) = oneshot::channel();
    drop(rx);
    hub.pending.lock().unwrap().insert(
        ("s1".into(), "perm-gone".into()),
        Pending {
            tool_name: "bash".into(),
            reply: tx,
        },
    );
    let (qtx, qrx) = oneshot::channel();
    drop(qrx);
    hub.pending_q.lock().unwrap().insert(
        ("s1".into(), "q-gone".into()),
        PendingQuestion {
            questions: Vec::new(),
            reply: qtx,
        },
    );
    hub.finish_run("s1");
}

#[tokio::test]
async fn permission_and_question_asks_time_out() {
    tokio::time::pause();
    let hub = PermHub::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    hub.register_run("s1", tx);
    let prompter = ServePrompter {
        hub: Arc::clone(&hub),
    };
    let s = scope("s1", false, Arc::clone(&hub));
    let task = tokio::spawn(async move {
        RUN.scope(s, async { prompter.ask("bash", "x").await })
            .await
    });
    let _ = rx.recv().await.expect("no permission ask");
    tokio::time::advance(Duration::from_secs(301)).await;
    assert!(!task.await.expect("ask task panicked"));

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    hub.register_run("s1", tx);
    let mut q = ServeQuestionPrompter::new(Arc::clone(&hub));
    q.timeout = Some(Duration::from_secs(300));
    let s = scope("s1", false, Arc::clone(&hub));
    let task = tokio::spawn(async move { RUN.scope(s, async { q.ask(Vec::new()).await }).await });
    let _ = rx.recv().await.expect("no question ask");
    tokio::time::advance(Duration::from_secs(301)).await;
    assert_eq!(
        task.await.expect("ask task panicked").unwrap_err(),
        QuestionError::Timeout
    );
}

#[tokio::test]
async fn question_ask_without_timeout_unblocks_and_can_notify() {
    let hub = PermHub::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    hub.register_run("s1", tx);
    let mut q = ServeQuestionPrompter::new(Arc::clone(&hub));
    q.timeout = None;
    q.notify = Some(Arc::new(whycodes_config::NotifyConfig {
        on: vec!["need_input".into()],
        ..Default::default()
    }));
    let s = scope("s1", false, Arc::clone(&hub));
    let task = tokio::spawn(async move { RUN.scope(s, async { q.ask(Vec::new()).await }).await });
    let ev = rx.recv().await.expect("no question ask");
    let TurnEvent::QuestionAsk { request_id, .. } = ev else {
        panic!("expected QuestionAsk");
    };
    hub.answer_question("s1", &request_id, None, false).unwrap();
    assert!(task.await.expect("ask task panicked").is_ok());

    let mut q = ServeQuestionPrompter::new(Arc::clone(&hub));
    q.timeout = None;
    let s = scope("s1", false, Arc::clone(&hub));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    hub.register_run("s1", tx);
    let task = tokio::spawn(async move { RUN.scope(s, async { q.ask(Vec::new()).await }).await });
    let _ = rx.recv().await.expect("no question ask");
    let pending = {
        let mut map = hub.pending_q.lock().unwrap();
        let key = map.keys().next().cloned().expect("pending key");
        map.remove(&key)
    };
    drop(pending);
    assert_eq!(
        task.await.expect("ask task panicked").unwrap_err(),
        QuestionError::Disconnected
    );
}

#[test]
fn is_always_allowed_is_false_without_an_entry() {
    let hub = PermHub::new();
    assert!(!hub.is_always_allowed("missing", "bash"));
}

#[tokio::test]
async fn timeout_cleanup_tolerates_poisoned_pending_maps() {
    tokio::time::pause();
    let hub = PermHub::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    hub.register_run("s1", tx);
    let prompter = ServePrompter {
        hub: Arc::clone(&hub),
    };
    let s = scope("s1", false, Arc::clone(&hub));
    let task = tokio::spawn(async move {
        RUN.scope(s, async { prompter.ask("bash", "x").await })
            .await
    });
    let _ = rx.recv().await.expect("no permission ask");
    poison_mutex(&hub.pending);
    tokio::time::advance(Duration::from_secs(301)).await;
    assert!(!task.await.expect("ask task panicked"));

    let hub = PermHub::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    hub.register_run("s1", tx);
    let mut q = ServeQuestionPrompter::new(Arc::clone(&hub));
    q.timeout = Some(Duration::from_secs(300));
    let s = scope("s1", false, Arc::clone(&hub));
    let task = tokio::spawn(async move { RUN.scope(s, async { q.ask(Vec::new()).await }).await });
    let _ = rx.recv().await.expect("no question ask");
    poison_mutex(&hub.pending_q);
    tokio::time::advance(Duration::from_secs(301)).await;
    assert_eq!(
        task.await.expect("ask task panicked").unwrap_err(),
        QuestionError::Timeout
    );
}
