use super::*;

fn msg(role: Role, content: MessageContent) -> Message {
    Message {
        role,
        content,
        tool_call_id: None,
        name: None,
        created_at: None,
    }
}

fn text(role: Role, s: &str) -> Message {
    msg(role, MessageContent::Text(s.into()))
}

fn tool_use() -> Message {
    msg(
        Role::Assistant,
        MessageContent::Blocks(vec![
            ContentBlock::Text {
                text: "running tests".into(),
            },
            ContentBlock::ToolUse {
                id: "t1".into(),
                name: "bash".into(),
                input: serde_json::json!({"command": "cargo test"}),
            },
        ]),
    )
}

fn tool_result(role: Role) -> Message {
    msg(
        role,
        MessageContent::Blocks(vec![ContentBlock::ToolResult {
            tool_use_id: "t1".into(),
            content: "test result: ok".into(),
            is_error: None,
        }]),
    )
}

#[test]
fn slash_parses_show_clear_set_and_too_long() {
    assert_eq!(parse_goal_slash("  "), GoalSlash::Show);
    for word in ["clear", "STOP", "off", "reset", "none", "cancel"] {
        assert_eq!(parse_goal_slash(word), GoalSlash::Clear, "{word}");
    }
    assert_eq!(
        parse_goal_slash(" all tests pass "),
        GoalSlash::Set("all tests pass".into())
    );
    let long = "x".repeat(MAX_CONDITION_CHARS + 1);
    assert_eq!(
        parse_goal_slash(&long),
        GoalSlash::TooLong(MAX_CONDITION_CHARS + 1)
    );
    assert_eq!(state_key("abc"), "goal:abc");
}

#[test]
fn idle_turns_pause_and_the_next_turn_resumes() {
    let mut goal = Goal::new("tests pass", 100);
    assert_eq!(goal.record_turn(false), TurnStep::Evaluate);
    assert!(goal.evaluating);
    assert_eq!(goal.record_turn(true), TurnStep::Evaluate);
    assert_eq!(goal.idle_turns, 0);
    goal.record_turn(false);
    goal.record_turn(false);
    assert_eq!(goal.record_turn(false), TurnStep::Pause);
    assert!(
        goal.paused
            .as_deref()
            .unwrap()
            .contains("no tool use for 3 turns")
    );
    assert!(!goal.evaluating);
    // The user's next prompt finishes a turn: the goal resumes.
    assert_eq!(goal.record_turn(true), TurnStep::Evaluate);
    assert!(goal.paused.is_none());
    assert_eq!(goal.turns, 6);
}

#[test]
fn status_shows_state_spend_and_last_reason() {
    let mut goal = Goal::new("lint is clean", 100);
    let fresh = goal.status(100);
    assert!(
        fresh.starts_with("◎ goal: lint is clean\nactive · 0s · 0 turns evaluated · 0 tokens"),
        "{fresh}"
    );
    goal.record_turn(true);
    goal.last_reason = Some("clippy still warns".into());
    let checking = goal.status(350);
    assert!(
        checking.contains("checking · 0s · 1 turn evaluated · 250 tokens"),
        "{checking}"
    );
    assert!(
        checking.ends_with("\nlast check: clippy still warns"),
        "{checking}"
    );
    goal.pause("turn failed");
    assert!(
        goal.status(350)
            .contains("paused (turn failed; send a prompt to resume)")
    );
    // Fewer tokens than at start (a resumed session) never underflows.
    assert!(goal.status(0).contains("· 0 tokens"));
    goal.check_tokens = 90;
    assert!(goal.status(350).contains("· 250 tokens + 90 for checks"));
}

#[test]
fn elapsed_label_picks_the_unit() {
    assert_eq!(elapsed_label(Duration::from_secs(42)), "42s");
    assert_eq!(elapsed_label(Duration::from_secs(125)), "2m 5s");
    assert_eq!(elapsed_label(Duration::from_secs(3_725)), "1h 2m");
}

#[test]
fn verdicts_parse_and_vague_answers_are_not_met() {
    assert_eq!(
        parse_verdict("VERDICT: met\nREASON: tests passed in the last output"),
        (Some(Verdict::Met), "tests passed in the last output".into())
    );
    assert_eq!(
        parse_verdict("verdict: **impossible**\nreason: the file does not exist"),
        (Some(Verdict::Impossible), "the file does not exist".into())
    );
    for not_met in ["not_met", "NOT MET", "unmet", "maybe"] {
        assert_eq!(
            parse_verdict(&format!("VERDICT: {not_met}\nREASON: lint fails")).0,
            Some(Verdict::NotMet),
            "{not_met}"
        );
    }
    // No verdict line: unreadable, and the last line is the reason.
    assert_eq!(
        parse_verdict("I think it is probably fine.\nNeeds a test run."),
        (None, "Needs a test run.".into())
    );
    assert_eq!(parse_verdict(""), (None, String::new()));
    assert_eq!(parse_verdict("verdicts are hard").0, None);
}

#[test]
fn verdicts_survive_the_markdown_small_models_add() {
    for answer in [
        "**VERDICT:** met\n**REASON:** all tests pass",
        "**VERDICT**: met\nREASON: all tests pass",
        "- VERDICT: met\n- REASON: all tests pass",
        "## Verdict = Met.\n> Reason - all tests pass",
        "  verdict:met\nreason:all tests pass",
        "VERDICT: **met**\nREASON: **all tests pass**",
    ] {
        assert_eq!(
            parse_verdict(answer),
            (Some(Verdict::Met), "all tests pass".into()),
            "{answer}"
        );
    }
    // Stars inside the reason are content, not markdown.
    assert_eq!(
        parse_verdict("VERDICT: met\nREASON: mul returns a * b now").1,
        "mul returns a * b now"
    );
}

#[test]
fn findings_keep_the_newest_and_turn_runs_restart_after_a_pause() {
    let mut goal = Goal::new("tests pass", 0);
    for turn in 1..=8 {
        goal.record_turn(true);
        goal.record_finding(&format!("reason {turn}"));
    }
    assert_eq!(goal.findings.len(), MAX_FINDINGS);
    assert_eq!(goal.findings[0], "after turn 3: reason 3");
    assert_eq!(goal.last_reason.as_deref(), Some("reason 8"));
    assert_eq!((goal.turns, goal.turns_this_run), (8, 8));
    goal.pause("turn cancelled");
    goal.record_turn(true);
    assert_eq!((goal.turns, goal.turns_this_run), (9, 1));
}

#[test]
fn turn_used_tools_looks_back_to_the_last_prompt() {
    let tool_turn = vec![
        text(Role::User, "fix it"),
        tool_use(),
        tool_result(Role::Tool),
        text(Role::Assistant, "done"),
    ];
    assert!(turn_used_tools(&tool_turn));
    // Tool results carried on a user message are not a new prompt.
    let user_results = vec![
        text(Role::User, "fix it"),
        tool_use(),
        tool_result(Role::User),
        text(Role::Assistant, "done"),
    ];
    assert!(turn_used_tools(&user_results));
    let chat_turn = vec![
        text(Role::User, "a"),
        tool_use(),
        text(Role::User, "b"),
        text(Role::Assistant, "ok"),
    ];
    assert!(
        !turn_used_tools(&chat_turn),
        "the earlier turn's tools do not count"
    );
    assert!(!turn_used_tools(&[]));
    assert!(!turn_used_tools(&[text(Role::Assistant, "hello")]));
}

#[test]
fn transcript_labels_roles_and_keeps_the_newest_text() {
    let messages = vec![
        text(Role::System, "system prompt"),
        text(Role::User, "make tests pass"),
        tool_use(),
        tool_result(Role::Tool),
        msg(
            Role::Assistant,
            MessageContent::Blocks(vec![ContentBlock::Thinking {
                text: "hidden".into(),
                signature: None,
            }]),
        ),
        text(Role::Assistant, "All green."),
    ];
    let t = transcript(&messages);
    assert!(t.starts_with("[user]\nmake tests pass"), "{t}");
    assert!(
        t.contains("(called bash {\"command\":\"cargo test\"})"),
        "{t}"
    );
    assert!(t.contains("[tool]\ntest result: ok"), "{t}");
    assert!(t.ends_with("[assistant]\nAll green."), "{t}");
    assert!(!t.contains("system prompt") && !t.contains("hidden"), "{t}");

    let big = "é".repeat(TRANSCRIPT_CHARS);
    let long = vec![
        text(Role::User, "first"),
        text(Role::Assistant, &big),
        text(Role::Assistant, "last"),
    ];
    let cut = transcript(&long);
    assert!(cut.len() <= TRANSCRIPT_CHARS, "{}", cut.len());
    assert!(cut.ends_with("last"));
    assert!(!cut.contains("first"));
}

#[test]
fn evaluator_request_has_no_tools_and_carries_goal_and_transcript() {
    let req = evaluator_request(
        "tests pass",
        7,
        &[],
        &[],
        &[text(Role::User, "go"), text(Role::Assistant, "ok")],
    );
    assert!(req.tools.is_empty());
    assert_eq!(req.temperature, Some(0.0));
    assert!(req.system.contains("VERDICT: met | not_met | impossible"));
    let body = req.messages[0].content.as_text().unwrap();
    assert!(
        body.starts_with("Goal:\ntests pass\n\nTurns taken toward this goal: 7\n\nTranscript"),
        "{body}"
    );
    assert!(body.ends_with("[assistant]\nok"), "{body}");

    let findings = vec!["after turn 1: build is green".to_string()];
    let req = evaluator_request("tests pass", 2, &findings, &[], &[text(Role::User, "go")]);
    let body = req.messages[0].content.as_text().unwrap();
    assert!(
        body.contains(
            "Earlier checks (oldest first):\n- after turn 1: build is green\n\nTranscript"
        ),
        "{body}"
    );

    use whycodes_core::todo::{TodoItem, TodoStatus};
    let todos = [
        TodoItem::new("1", "fix parser", TodoStatus::Completed),
        TodoItem::new("2", "run tests", TodoStatus::Pending),
    ];
    let req = evaluator_request("tests pass", 2, &findings, &todos, &[]);
    let body = req.messages[0].content.as_text().unwrap();
    assert!(
        body.contains("build is green\n\nAgent's todo list:\n☑ fix parser (completed)\n☐ run tests (pending)\n\nTranscript"),
        "{body}"
    );
    assert!(req.system.contains("todo list is its own plan"));
}

#[test]
fn continue_prompt_names_goal_reason_and_evidence() {
    let p = continue_prompt("tests pass", "two tests still fail");
    assert!(p.starts_with("Continue working toward the goal: tests pass\n"));
    assert!(p.contains("not met yet — two tests still fail"));
    assert!(p.contains("show the evidence"));
}

#[test]
fn checker_model_prefers_goal_model_then_the_session_model() {
    let pick = |goal_model| checker_model("openrouter", "big", goal_model);
    assert_eq!(pick(None), ("openrouter".into(), "big".into()));
    assert_eq!(pick(Some("  ")), ("openrouter".into(), "big".into()));
    assert_eq!(pick(Some("judge")), ("openrouter".into(), "judge".into()));
    assert_eq!(
        pick(Some("aiproxy/gc/grok-4.7")),
        ("aiproxy".into(), "gc/grok-4.7".into())
    );
}

#[test]
fn goal_turns_ask_for_the_intent_and_an_honest_stop() {
    let start = goal_prompt("tests pass");
    assert!(start.starts_with("Goal: tests pass\n"), "{start}");
    for prompt in [start, continue_prompt("tests pass", "red")] {
        assert!(prompt.contains("as the user means it"), "{prompt}");
        assert!(prompt.contains("instead of working around it"), "{prompt}");
    }
}
