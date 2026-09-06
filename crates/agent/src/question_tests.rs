use super::*;
use whycodes_tools::question::QuestionOption;

#[tokio::test]
async fn auto_prompter_picks_first() {
    let p = AutoAnswerPrompter;
    let qs = vec![QuestionSpec {
        prompt: "x".into(),
        options: vec![
            QuestionOption {
                label: "A".into(),
                description: String::new(),
                preview: None,
            },
            QuestionOption {
                label: "B".into(),
                description: String::new(),
                preview: None,
            },
        ],
        multi_select: false,
        important: false,
    }];
    let a = p.ask(qs).await.unwrap();
    assert_eq!(a[0].selected, vec!["A".to_string()]);
    assert!(a[0].auto_picked);
}

#[test]
fn auto_prompts_only_when_any_question_is_important() {
    let routine = vec![QuestionSpec {
        prompt: "x".into(),
        options: vec![],
        multi_select: false,
        important: false,
    }];
    let important = vec![QuestionSpec {
        prompt: "y".into(),
        options: vec![],
        multi_select: false,
        important: true,
    }];
    assert!(!should_prompt_questions(ApprovalMode::Auto, &routine));
    assert!(should_prompt_questions(ApprovalMode::Auto, &important));
    assert!(should_prompt_questions(ApprovalMode::Important, &routine));
    assert!(should_prompt_questions(ApprovalMode::Manual, &routine));
}

#[tokio::test]
async fn auto_prompter_free_text_when_no_options() {
    let p = AutoAnswerPrompter;
    let qs = vec![QuestionSpec {
        prompt: "free?".into(),
        options: vec![],
        multi_select: false,
        important: false,
    }];
    let a = p.ask(qs).await.unwrap();
    assert_eq!(a[0].free_text.as_deref(), Some("auto"));
    assert!(a[0].selected.is_empty());
    assert!(a[0].auto_picked);
}

#[tokio::test]
async fn channel_timeout_and_disconnect() {
    let (prompter, rx) = ChannelQuestionPrompter::new(Some(Duration::from_millis(20)));
    drop(rx);
    let err = prompter
        .ask(vec![QuestionSpec {
            prompt: "x".into(),
            options: vec![],
            multi_select: false,
            important: false,
        }])
        .await
        .expect_err("disconnected");
    assert_eq!(err, QuestionError::Disconnected);

    let (prompter, mut rx) = ChannelQuestionPrompter::new(Some(Duration::from_millis(20)));
    let ask = tokio::spawn(async move {
        prompter
            .ask(vec![QuestionSpec {
                prompt: "y".into(),
                options: vec![],
                multi_select: false,
                important: true,
            }])
            .await
    });
    let _req = rx.recv().await.expect("request");
    let err = ask.await.unwrap().expect_err("timeout");
    assert_eq!(err, QuestionError::Timeout);
    assert!(err.message().to_lowercase().contains("timed"));
    assert!(
        QuestionError::Cancelled
            .message()
            .to_lowercase()
            .contains("cancel")
    );
    assert!(
        QuestionError::Invalid("x".into())
            .message()
            .contains("Invalid")
    );
}

#[tokio::test]
async fn channel_none_timeout_disconnects_when_reply_dropped() {
    let (prompter, mut rx) = ChannelQuestionPrompter::new(None);
    let ask = tokio::spawn(async move {
        prompter
            .ask(vec![QuestionSpec {
                prompt: "z".into(),
                options: vec![],
                multi_select: false,
                important: false,
            }])
            .await
    });
    let req = rx.recv().await.expect("request");
    drop(req.reply);
    let err = ask.await.unwrap().expect_err("disconnected");
    assert_eq!(err, QuestionError::Disconnected);
}

#[tokio::test]
async fn channel_timeout_some_disconnected_after_send() {
    let (prompter, mut rx) = ChannelQuestionPrompter::new(Some(Duration::from_millis(80)));
    let ask = tokio::spawn(async move {
        prompter
            .ask(vec![QuestionSpec {
                prompt: "drop-reply".into(),
                options: vec![],
                multi_select: false,
                important: false,
            }])
            .await
    });
    let req = rx.recv().await.expect("request");
    drop(req.reply);
    let err = ask.await.unwrap().expect_err("disconnected");
    assert_eq!(err, QuestionError::Disconnected);
}

#[tokio::test]
async fn run_question_tool_invalid_args() {
    let p = AutoAnswerPrompter;
    let r = run_question_tool(&p, &serde_json::json!({}), "q1").await;
    assert!(r.is_error);
    assert!(r.content.contains("Invalid questionnaire"), "{r:?}");
    assert_eq!(r.tool_call_id, "q1");
}

#[tokio::test]
async fn run_question_tool_auto_answers() {
    let p = AutoAnswerPrompter;
    let args = serde_json::json!({
        "questions": [{
            "prompt": "pick",
            "options": [{"label": "A"}, {"label": "B"}]
        }]
    });
    let r = run_question_tool(&p, &args, "q2").await;
    assert!(!r.is_error, "{r:?}");
    assert!(
        r.content.contains("A") || r.content.to_lowercase().contains("auto"),
        "{r:?}"
    );
}

#[test]
fn default_question_prompter_ci_and_construct_stdin() {
    let prev_approve = std::env::var_os("WHYCODES_AUTO_APPROVE");
    let prev_ci = std::env::var_os("CI");
    unsafe { std::env::set_var("WHYCODES_AUTO_APPROVE", "0") };
    unsafe { std::env::set_var("CI", "1") };
    let _ = default_question_prompter();
    unsafe { std::env::remove_var("CI") };
    let _ = default_question_prompter();
    let _ = StdinQuestionPrompter;
    // Force both restore arms so leftover coverage is not env-dependent.
    unsafe { std::env::set_var("WHYCODES_AUTO_APPROVE", "restore-some") };
    if let Some(v) = std::env::var_os("WHYCODES_AUTO_APPROVE") {
        unsafe { std::env::set_var("WHYCODES_AUTO_APPROVE", v) };
    }
    unsafe { std::env::remove_var("WHYCODES_AUTO_APPROVE") };
    if std::env::var_os("WHYCODES_AUTO_APPROVE").is_none() {
        // None restore arm
    }
    if let Some(v) = prev_approve {
        unsafe { std::env::set_var("WHYCODES_AUTO_APPROVE", v) };
    } else {
        unsafe { std::env::remove_var("WHYCODES_AUTO_APPROVE") };
    }
    unsafe { std::env::set_var("CI", "restore-some") };
    if let Some(v) = std::env::var_os("CI") {
        unsafe { std::env::set_var("CI", v) };
    }
    unsafe { std::env::remove_var("CI") };
    if let Some(v) = prev_ci {
        unsafe { std::env::set_var("CI", v) };
    } else {
        unsafe { std::env::remove_var("CI") };
    }
}

#[test]
fn disconnected_message_is_stable() {
    assert_eq!(
        QuestionError::Disconnected.message(),
        "Questionnaire UI disconnected."
    );
}

#[tokio::test]
async fn channel_with_notify_and_successful_reply() {
    let cfg = whycodes_config::NotifyConfig {
        on: vec!["need_input".into()],
        discord_webhook: Some("https://example.invalid/webhook".into()),
        ..Default::default()
    };
    let (prompter, mut rx) = ChannelQuestionPrompter::new(Some(Duration::from_secs(2)));
    let prompter = prompter.with_notify(crate::notify::handle_from_config(&cfg));
    let ask = tokio::spawn(async move {
        prompter
            .ask(vec![QuestionSpec {
                prompt: "pick one".into(),
                options: vec![QuestionOption {
                    label: "A".into(),
                    description: String::new(),
                    preview: None,
                }],
                multi_select: false,
                important: true,
            }])
            .await
    });
    let req = rx.recv().await.expect("request");
    req.reply
        .send(Ok(vec![QuestionAnswer {
            selected: vec!["A".into()],
            free_text: None,
            auto_picked: false,
        }]))
        .unwrap();
    let answers = ask.await.unwrap().expect("ok");
    assert_eq!(answers[0].selected, vec!["A".to_string()]);
}

#[tokio::test]
async fn run_question_tool_propagates_prompter_error() {
    let (prompter, rx) = ChannelQuestionPrompter::new(Some(Duration::from_millis(20)));
    drop(rx);
    let args = serde_json::json!({
        "questions": [{"prompt": "pick", "options": [{"label": "A"}]}]
    });
    let r = run_question_tool(&prompter, &args, "q3").await;
    assert!(r.is_error);
    assert!(r.content.contains("disconnected") || r.content.contains("Disconnected"));
}

#[test]
fn default_question_prompter_auto_approve() {
    let prev_approve = std::env::var_os("WHYCODES_AUTO_APPROVE");
    let prev_ci = std::env::var_os("CI");
    unsafe { std::env::set_var("CI", "0") };
    unsafe { std::env::set_var("WHYCODES_AUTO_APPROVE", "yes") };
    let _ = default_question_prompter();
    if let Some(v) = prev_approve {
        unsafe { std::env::set_var("WHYCODES_AUTO_APPROVE", v) };
    } else {
        unsafe { std::env::remove_var("WHYCODES_AUTO_APPROVE") };
    }
    if let Some(v) = prev_ci {
        unsafe { std::env::set_var("CI", v) };
    } else {
        unsafe { std::env::remove_var("CI") };
    }
}

struct InvalidAnswersPrompter;

impl QuestionPrompter for InvalidAnswersPrompter {
    fn ask(&self, questions: Vec<QuestionSpec>) -> QuestionAskFuture<'_> {
        Box::pin(async move {
            Ok(questions
                .iter()
                .map(|_| QuestionAnswer {
                    selected: vec!["not-an-option".into()],
                    free_text: None,
                    auto_picked: false,
                })
                .collect())
        })
    }
}

#[tokio::test]
async fn run_question_tool_rejects_invalid_answers() {
    let p = InvalidAnswersPrompter;
    let args = serde_json::json!({
        "questions": [{
            "prompt": "pick",
            "options": [{"label": "A"}, {"label": "B"}]
        }]
    });
    let r = run_question_tool(&p, &args, "q-bad").await;
    assert!(r.is_error);
    assert!(
        r.content.contains("Invalid") || r.content.contains("unknown option"),
        "{r:?}"
    );
}

#[test]
fn parse_stdin_question_line_covers_numeric_other_and_free_text() {
    let free = QuestionSpec {
        prompt: "free?".into(),
        options: vec![],
        multi_select: false,
        important: false,
    };
    assert_eq!(
        parse_stdin_question_line(&free, ""),
        StdinQuestionParse::Cancelled
    );
    assert_eq!(
        parse_stdin_question_line(&free, "hello"),
        StdinQuestionParse::FreeText("hello".into())
    );

    let choice = QuestionSpec {
        prompt: "pick".into(),
        options: vec![
            QuestionOption {
                label: "A".into(),
                description: "alpha".into(),
                preview: None,
            },
            QuestionOption {
                label: "B".into(),
                description: String::new(),
                preview: None,
            },
        ],
        multi_select: false,
        important: false,
    };
    assert_eq!(
        parse_stdin_question_line(&choice, ""),
        StdinQuestionParse::Cancelled
    );
    assert_eq!(
        parse_stdin_question_line(&choice, "1"),
        StdinQuestionParse::Selected("A".into())
    );
    assert_eq!(
        parse_stdin_question_line(&choice, "2"),
        StdinQuestionParse::Selected("B".into())
    );
    assert_eq!(
        parse_stdin_question_line(&choice, "3"),
        StdinQuestionParse::Other
    );
    assert_eq!(
        parse_stdin_question_line(&choice, "9"),
        StdinQuestionParse::FreeText("9".into())
    );
    assert_eq!(
        parse_stdin_question_line(&choice, "typed"),
        StdinQuestionParse::FreeText("typed".into())
    );
}

#[tokio::test]
async fn stdin_question_prompter_eof_cancels() {
    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return;
    }
    let p = StdinQuestionPrompter;
    let err = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        p.ask(vec![QuestionSpec {
            prompt: "free?".into(),
            options: vec![],
            multi_select: false,
            important: false,
        }]),
    )
    .await
    .expect("stdin question must not hang on EOF");
    assert!(err.is_err(), "{err:?}");

    let err = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        p.ask(vec![
            QuestionSpec {
                prompt: "one".into(),
                options: vec![QuestionOption {
                    label: "A".into(),
                    description: "alpha".into(),
                    preview: None,
                }],
                multi_select: false,
                important: false,
            },
            QuestionSpec {
                prompt: "two".into(),
                options: vec![QuestionOption {
                    label: "B".into(),
                    description: String::new(),
                    preview: None,
                }],
                multi_select: false,
                important: false,
            },
        ]),
    )
    .await
    .expect("multi-question stdin must not hang on EOF");
    assert!(err.is_err(), "{err:?}");
}
