use super::*;
use whycodes_core::TodoItem;

fn msg(role: Role, content: MessageContent) -> Message {
    Message {
        role,
        content,
        tool_call_id: None,
        name: None,
        created_at: None,
    }
}

#[test]
fn text_thinking_and_tool_events_map_to_updates() {
    let cwd = Path::new("/proj");
    assert_eq!(
        update_for(&TurnEvent::TextDelta("hi".into()), cwd).unwrap(),
        json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "hi"}})
    );
    assert_eq!(
        update_for(&TurnEvent::ThinkingDelta("hm".into()), cwd).unwrap()["sessionUpdate"],
        "agent_thought_chunk"
    );
    let start = update_for(
        &TurnEvent::ToolStart {
            id: "t1".into(),
            name: "read".into(),
            input: json!({"path": "src/a.rs"}),
        },
        cwd,
    )
    .unwrap();
    assert_eq!(start["sessionUpdate"], "tool_call");
    assert_eq!(start["toolCallId"], "t1");
    assert_eq!(start["title"], "read: src/a.rs");
    assert_eq!(start["kind"], "read");
    assert_eq!(start["status"], "in_progress");
    assert!(
        start["locations"][0]["path"]
            .as_str()
            .unwrap()
            .ends_with("a.rs")
    );
    let bare = update_for(
        &TurnEvent::ToolStart {
            id: "t2".into(),
            name: "task".into(),
            input: json!({}),
        },
        cwd,
    )
    .unwrap();
    assert_eq!(bare["locations"], json!([]));

    let failed = update_for(
        &TurnEvent::ToolEnd {
            id: "t1".into(),
            content: "x".repeat(MAX_TOOL_OUTPUT + 5),
            is_error: true,
        },
        cwd,
    )
    .unwrap();
    assert_eq!(failed["status"], "failed");
    let text = failed["content"][0]["content"]["text"].as_str().unwrap();
    assert_eq!(text.chars().count(), MAX_TOOL_OUTPUT + 1);
    let done = update_for(
        &TurnEvent::ToolEnd {
            id: "t1".into(),
            content: "ok".into(),
            is_error: false,
        },
        cwd,
    )
    .unwrap();
    assert_eq!(done["status"], "completed");
    assert!(update_for(&TurnEvent::Status("s".into()), cwd).is_none());
}

#[test]
fn todos_become_a_plan_without_cancelled_items() {
    let todos = vec![
        TodoItem::new("1", "read", TodoStatus::Completed),
        TodoItem::new("2", "edit", TodoStatus::InProgress),
        TodoItem::new("3", "test", TodoStatus::Pending),
        TodoItem::new("4", "dropped", TodoStatus::Cancelled),
    ];
    let plan = update_for(&TurnEvent::Todos { todos }, Path::new("/")).unwrap();
    assert_eq!(plan["sessionUpdate"], "plan");
    let entries = plan["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0]["status"], "completed");
    assert_eq!(entries[1]["status"], "in_progress");
    assert_eq!(entries[2]["status"], "pending");
    assert_eq!(entries[2]["priority"], "medium");
}

#[test]
fn tool_kinds_and_titles() {
    for (name, kind) in [
        ("git_diff", "read"),
        ("grep", "search"),
        ("apply_patch", "edit"),
        ("bash", "execute"),
        ("webfetch", "fetch"),
        ("plan", "think"),
        ("task", "other"),
    ] {
        assert_eq!(tool_kind(name), kind, "{name}");
    }
    assert_eq!(
        tool_title("bash", &json!({"command": "cargo test"})),
        "bash: cargo test"
    );
    let long = tool_title("grep", &json!({"pattern": "p".repeat(200)}));
    assert!(long.ends_with('…'));
    assert_eq!(tool_title("task", &json!({"n": 1})), "task");
}

#[test]
fn prompt_blocks_become_text() {
    let blocks = vec![
        json!({"type": "text", "text": "fix"}),
        json!({"type": "text"}),
        json!({"type": "resource_link", "name": "a", "uri": "file:///home/u/a.rs"}),
        json!({"type": "resource_link", "name": "w", "uri": "file:///C:/p/w.rs"}),
        json!({"type": "resource_link", "name": "x"}),
        json!({"type": "resource", "resource": {"uri": "file:///b.rs", "text": "fn b() {}"}}),
        json!({"type": "resource", "resource": {"uri": "https://x.dev/doc", "blob": "AA=="}}),
        json!({"type": "resource", "resource": {"blob": "AA=="}}),
        json!({"type": "image", "data": "AA==", "mimeType": "image/png"}),
    ];
    assert_eq!(
        prompt_text(&blocks),
        "fix\n@/home/u/a.rs\n@C:/p/w.rs\n<context uri=\"file:///b.rs\">\nfn b() {}\n</context>\nhttps://x.dev/doc"
    );
}

#[test]
fn history_replays_user_and_assistant_text() {
    let messages = vec![
        msg(Role::System, MessageContent::Text("sys".into())),
        msg(Role::User, MessageContent::Text("hello".into())),
        msg(
            Role::Assistant,
            MessageContent::Blocks(vec![
                ContentBlock::Text { text: "one".into() },
                ContentBlock::ToolUse {
                    id: "t".into(),
                    name: "read".into(),
                    input: json!({}),
                },
                ContentBlock::Text { text: "two".into() },
            ]),
        ),
        msg(Role::Tool, MessageContent::Text("out".into())),
        msg(Role::Assistant, MessageContent::Text("  ".into())),
    ];
    let updates = history_updates(&messages);
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0]["sessionUpdate"], "user_message_chunk");
    assert_eq!(updates[0]["content"]["text"], "hello");
    assert_eq!(updates[1]["sessionUpdate"], "agent_message_chunk");
    assert_eq!(updates[1]["content"]["text"], "one\ntwo");
}
