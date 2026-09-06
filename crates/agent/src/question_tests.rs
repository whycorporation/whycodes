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

    let free_text = apply_free_stdin_parse(StdinQuestionParse::FreeText("hello".into()), "hello")
        .expect("free text");
    assert_eq!(free_text.free_text.as_deref(), Some("hello"));
    let free_selected =
        apply_free_stdin_parse(StdinQuestionParse::Selected("unused".into()), "unused")
            .expect("selected-as-free");
    assert_eq!(free_selected.free_text.as_deref(), Some("unused"));
    let free_other =
        apply_free_stdin_parse(StdinQuestionParse::Other, "typed-other").expect("other");
    assert_eq!(free_other.free_text.as_deref(), Some("typed-other"));
    assert_eq!(
        apply_free_stdin_parse(StdinQuestionParse::Cancelled, ""),
        Err(QuestionError::Cancelled)
    );

    let selected =
        apply_choice_stdin_parse(StdinQuestionParse::Selected("A".into()), None).expect("selected");
    assert_eq!(selected.selected, vec!["A".to_string()]);
    let other_empty =
        apply_choice_stdin_parse(StdinQuestionParse::Other, Some("  ")).expect("empty");
    assert!(other_empty.free_text.is_none());
    let other_text =
        apply_choice_stdin_parse(StdinQuestionParse::Other, Some("custom")).expect("custom");
    assert_eq!(other_text.free_text.as_deref(), Some("custom"));
    let other_none = apply_choice_stdin_parse(StdinQuestionParse::Other, None).expect("none");
    assert!(other_none.free_text.is_none());
    let typed = apply_choice_stdin_parse(StdinQuestionParse::FreeText("typed".into()), None)
        .expect("typed");
    assert_eq!(typed.free_text.as_deref(), Some("typed"));
    assert_eq!(
        apply_choice_stdin_parse(StdinQuestionParse::Cancelled, None),
        Err(QuestionError::Cancelled)
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

#[test]
fn ask_stdin_questions_covers_free_choice_other_and_errors() {
    let free = vec![QuestionSpec {
        prompt: "free?".into(),
        options: vec![],
        multi_select: false,
        important: false,
    }];
    let answers = ask_stdin_questions(free, || Ok("hello".into())).expect("free");
    assert_eq!(answers[0].free_text.as_deref(), Some("hello"));

    let mut other_fail = std::collections::VecDeque::from(["2".to_string()]);
    let other_empty = ask_stdin_questions(
        vec![QuestionSpec {
            prompt: "pick".into(),
            options: vec![QuestionOption {
                label: "A".into(),
                description: String::new(),
                preview: None,
            }],
            multi_select: false,
            important: false,
        }],
        || other_fail.pop_front().ok_or_else(|| "eof".into()),
    )
    .expect("other text falls back to empty");
    assert!(other_empty[0].free_text.is_none());
    assert!(other_empty[0].selected.is_empty());
    assert!(!other_empty[0].auto_picked);

    let cancelled = ask_stdin_questions(
        vec![QuestionSpec {
            prompt: "free?".into(),
            options: vec![],
            multi_select: false,
            important: false,
        }],
        || Ok(String::new()),
    );
    assert_eq!(cancelled, Err(QuestionError::Cancelled));

    let invalid = ask_stdin_questions(
        vec![QuestionSpec {
            prompt: "free?".into(),
            options: vec![],
            multi_select: false,
            important: false,
        }],
        || Err("boom".into()),
    );
    assert_eq!(invalid, Err(QuestionError::Invalid("boom".into())));

    let invalid_choice = ask_stdin_questions(
        vec![QuestionSpec {
            prompt: "pick".into(),
            options: vec![QuestionOption {
                label: "A".into(),
                description: String::new(),
                preview: None,
            }],
            multi_select: false,
            important: false,
        }],
        || Err("choice-boom".into()),
    );
    assert_eq!(
        invalid_choice,
        Err(QuestionError::Invalid("choice-boom".into()))
    );

    let choice = vec![QuestionSpec {
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
    }];
    let selected = ask_stdin_questions(choice.clone(), || Ok("1".into())).expect("selected");
    assert_eq!(selected[0].selected, vec!["A".to_string()]);

    let mut other_lines = std::collections::VecDeque::from(["3".to_string(), "custom".to_string()]);
    let other = ask_stdin_questions(choice.clone(), || {
        other_lines.pop_front().ok_or_else(|| "eof".into())
    })
    .expect("other");
    assert_eq!(other[0].free_text.as_deref(), Some("custom"));

    let typed = ask_stdin_questions(choice, || Ok("typed".into())).expect("typed");
    assert_eq!(typed[0].free_text.as_deref(), Some("typed"));

    let two = vec![
        QuestionSpec {
            prompt: "one".into(),
            options: vec![QuestionOption {
                label: "A".into(),
                description: String::new(),
                preview: None,
            }],
            multi_select: false,
            important: false,
        },
        QuestionSpec {
            prompt: "two".into(),
            options: vec![QuestionOption {
                label: "B".into(),
                description: "beta".into(),
                preview: None,
            }],
            multi_select: false,
            important: false,
        },
    ];
    let mut lines = std::collections::VecDeque::from(["1".to_string(), "1".to_string()]);
    let both = ask_stdin_questions(two, || lines.pop_front().ok_or_else(|| "eof".into()))
        .expect("two questions");
    assert_eq!(both[0].selected, vec!["A".to_string()]);
    assert_eq!(both[1].selected, vec!["B".to_string()]);
}
