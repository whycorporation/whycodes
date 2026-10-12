use super::*;

fn user(text: &str) -> Message {
    Message {
        role: Role::User,
        content: MessageContent::Text(text.into()),
        tool_call_id: None,
        name: None,
        created_at: None,
    }
}

fn calls(ids: &[&str]) -> Message {
    let mut blocks = vec![ContentBlock::Text {
        text: "working".into(),
    }];
    for id in ids {
        blocks.push(ContentBlock::ToolUse {
            id: (*id).into(),
            name: "read".into(),
            input: serde_json::json!({}),
        });
    }
    Message {
        role: Role::Assistant,
        content: MessageContent::Blocks(blocks),
        tool_call_id: None,
        name: None,
        created_at: None,
    }
}

fn result(id: &str) -> Message {
    Message {
        role: Role::Tool,
        content: MessageContent::Text(format!("out {id}")),
        tool_call_id: Some(id.into()),
        name: None,
        created_at: None,
    }
}

fn shape(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .map(|m| match m.role {
            Role::Tool => {
                let id = m.tool_call_id.clone().unwrap_or_default();
                if m.content.as_text() == Some(MISSING_RESULT) {
                    format!("missing:{id}")
                } else {
                    format!("tool:{id}")
                }
            }
            Role::Assistant => "assistant".into(),
            Role::User => "user".into(),
            Role::System => "system".into(),
        })
        .collect()
}

#[test]
fn a_paired_transcript_is_left_alone() {
    let messages = vec![
        user("q"),
        calls(&["a", "b"]),
        result("a"),
        result("b"),
        calls(&[]),
        user("next"),
    ];
    assert!(paired_messages(&messages).is_none());
    assert!(paired_messages(&[]).is_none());
}

#[test]
fn unanswered_calls_get_a_result_right_after_their_turn() {
    // Cancelled mid-batch: `b` answered, `a` and `c` never ran; then the user
    // typed again.
    let messages = vec![
        user("q"),
        calls(&["a", "b", "c"]),
        result("b"),
        user("next"),
    ];
    let fixed = paired_messages(&messages).expect("repaired");
    assert_eq!(
        shape(&fixed),
        [
            "user",
            "assistant",
            "tool:b",
            "missing:a",
            "missing:c",
            "user"
        ]
    );
}

#[test]
fn a_trailing_unanswered_call_and_a_new_assistant_turn_are_closed() {
    let messages = vec![user("q"), calls(&["a"]), calls(&["b"])];
    let fixed = paired_messages(&messages).expect("repaired");
    assert_eq!(
        shape(&fixed),
        ["user", "assistant", "missing:a", "assistant", "missing:b"]
    );
}

#[test]
fn results_that_answer_no_call_are_dropped() {
    let messages = vec![
        user("q"),
        result("ghost"),
        calls(&["a"]),
        result("a"),
        result("a"),
    ];
    let fixed = paired_messages(&messages).expect("repaired");
    assert_eq!(shape(&fixed), ["user", "assistant", "tool:a"]);
}

#[test]
fn results_after_a_call_without_an_id_are_kept() {
    let messages = vec![user("q"), calls(&[""]), result("call_0")];
    assert!(paired_messages(&messages).is_none());

    let text_only = Message {
        role: Role::Assistant,
        content: MessageContent::Text("plain".into()),
        tool_call_id: None,
        name: None,
        created_at: None,
    };
    let system = Message {
        role: Role::System,
        ..user("rules")
    };
    let messages = vec![system, text_only, result("stray")];
    let fixed = paired_messages(&messages).expect("repaired");
    assert_eq!(shape(&fixed), ["system", "assistant"]);
}

#[test]
fn build_request_sends_the_repaired_copy_and_keeps_the_transcript() {
    let mut session = crate::session::Session::new(std::path::PathBuf::from("/p"), "sys".into());
    session.messages = vec![user("q"), calls(&["a"]), user("next")];
    let request = session.build_request(Vec::new(), None, None, None);
    assert_eq!(
        shape(&request.messages),
        ["user", "assistant", "missing:a", "user"]
    );
    assert_eq!(shape(&session.messages), ["user", "assistant", "user"]);
}
