use super::*;
use std::path::PathBuf;
use whycodes_core::types::ImageSource;

fn session() -> Session {
    Session::new(PathBuf::from("/tmp/tree"), "sys".into())
}

fn msg(role: Role, content: MessageContent) -> Message {
    Message {
        role,
        content,
        tool_call_id: None,
        name: None,
        created_at: None,
    }
}

/// One user turn: the prompt and a text reply.
fn turn(s: &mut Session, prompt: &str) {
    s.add_user_message(prompt);
    s.add_assistant_message(vec![ContentBlock::Text {
        text: format!("re: {prompt}"),
    }]);
}

fn labels(rows: &[TreeRow]) -> Vec<String> {
    rows.iter().map(TreeRow::line).collect()
}

fn active_texts(s: &Session) -> Vec<String> {
    s.messages
        .iter()
        .filter(|m| m.role == Role::User)
        .filter_map(|m| m.content.as_text().map(str::to_string))
        .collect()
}

fn row<'a>(rows: &'a [TreeRow], label: &str) -> &'a TreeRow {
    rows.iter().find(|r| r.label == label).unwrap()
}

#[test]
fn a_linear_conversation_is_one_flat_line() {
    let mut s = session();
    assert!(s.tree_rows().is_empty());
    turn(&mut s, "one");
    turn(&mut s, "two");
    turn(&mut s, "three");
    let rows = s.tree_rows();
    assert_eq!(labels(&rows), ["• one", "• two", "● three"]);
    assert!(rows.iter().all(|r| r.on_active_path && r.depth == 0));
    assert!(rows[2].active_leaf && !rows[1].active_leaf);
}

#[test]
fn mark_branch_names_the_current_point() {
    let mut s = session();
    assert_eq!(s.mark_branch("x"), Err(BranchError::Empty));
    turn(&mut s, "one");
    assert_eq!(s.mark_branch("  ").unwrap(), "branch 1");
    turn(&mut s, "two");
    assert_eq!(s.mark_branch(" idea ").unwrap(), "idea");
    let rows = s.tree_rows();
    assert_eq!(labels(&rows), ["• one  [branch 1]", "● two  [idea]"]);
    assert_eq!(s.branches.len(), 2);
}

#[test]
fn switching_to_an_earlier_turn_keeps_the_old_path_as_a_sibling() {
    let mut s = session();
    turn(&mut s, "one");
    turn(&mut s, "two");
    turn(&mut s, "three");
    s.mark_checkpoint("explore");

    let target = row(&s.tree_rows(), "one").target;
    assert_eq!(s.switch_branch(target), Ok(true));
    assert_eq!(active_texts(&s), ["one"]);
    assert_eq!(s.messages.len(), 2);
    assert!(s.checkpoint.is_none());
    assert_eq!(s.branches.len(), 1);
    assert!(s.branches[0].name.is_none());

    // The active leaf is now inside the stored path.
    let rows = s.tree_rows();
    assert_eq!(labels(&rows), ["● one", "○ two", "○ three"]);

    // A new turn appends a child next to the old one.
    turn(&mut s, "alt");
    let rows = s.tree_rows();
    assert_eq!(labels(&rows), ["• one", "↳ ● alt", "↳ ○ two", "  ○ three"]);
    assert!(row(&rows, "alt").fork_start && row(&rows, "two").fork_start);
    assert!(!row(&rows, "three").fork_start);
    assert_eq!(row(&rows, "three").depth, 1);

    // Back to the old leaf: that unnamed branch becomes active again and
    // the alternative is kept.
    let target = row(&rows, "three").target;
    assert_eq!(s.switch_branch(target), Ok(true));
    assert_eq!(active_texts(&s), ["one", "two", "three"]);
    assert_eq!(s.branches.len(), 1);
    assert_eq!(s.branches[0].messages.len(), 4);
    let rows = s.tree_rows();
    assert_eq!(labels(&rows), ["• one", "↳ • two", "  ● three", "↳ ○ alt"]);
}

#[test]
fn a_path_held_by_another_is_not_stored_twice() {
    let mut s = session();
    turn(&mut s, "one");
    turn(&mut s, "two");
    let one = row(&s.tree_rows(), "one").target;
    s.switch_branch(one).unwrap();
    assert_eq!(s.branches.len(), 1);
    // The active [one] is a prefix of the stored path: going to its leaf
    // moves it back and stores nothing new.
    let two = row(&s.tree_rows(), "two").target;
    s.switch_branch(two).unwrap();
    assert!(s.branches.is_empty());
    assert_eq!(active_texts(&s), ["one", "two"]);
}

#[test]
fn a_named_branch_stays_marked_after_switching_to_it() {
    let mut s = session();
    turn(&mut s, "one");
    s.mark_branch("keep").unwrap();
    turn(&mut s, "two");
    let one = row(&s.tree_rows(), "one").target;
    // `one` on the active path: the longer path is stored, the bookmark stays.
    s.switch_branch(one).unwrap();
    assert_eq!(s.branches.len(), 2);
    let rows = s.tree_rows();
    assert_eq!(labels(&rows), ["● one  [keep]", "○ two"]);

    // Selecting the bookmark itself, which is the active path, is a no-op.
    let keep = TreeTarget { path: 1, end: 2 };
    assert_eq!(s.switch_branch(keep), Ok(false));
    assert_eq!(s.branches.len(), 2);
}

#[test]
fn stale_targets_are_refused() {
    let mut s = session();
    turn(&mut s, "one");
    let before = s.messages.len();
    assert_eq!(
        s.switch_branch(TreeTarget { path: 3, end: 1 }),
        Err(BranchError::Stale)
    );
    assert_eq!(
        s.switch_branch(TreeTarget { path: 0, end: 9 }),
        Err(BranchError::Stale)
    );
    assert_eq!(s.messages.len(), before);
    assert_eq!(
        BranchError::Stale.to_string(),
        "the conversation changed; open /tree again"
    );
    assert_eq!(BranchError::Empty.to_string(), "nothing to branch from yet");
}

#[test]
fn pruned_tool_bodies_still_match_their_branch() {
    let mut s = session();
    s.add_user_message("run it");
    s.add_assistant_message(vec![ContentBlock::ToolUse {
        id: "c1".into(),
        name: "bash".into(),
        input: serde_json::json!({"cmd": "ls"}),
    }]);
    s.add_tool_results(vec![whycodes_core::types::ToolResult {
        tool_call_id: "c1".into(),
        content: "a long body".into(),
        is_error: false,
    }]);
    turn(&mut s, "next");
    s.mark_branch("snap").unwrap();
    // Shrink the tool body in place on the active path only.
    s.messages[2].content = MessageContent::text("[pruned]");
    let rows = s.tree_rows();
    assert_eq!(labels(&rows), ["• run it", "● next  [snap]"]);
}

#[test]
fn a_compacted_path_forks_from_the_root() {
    let mut s = session();
    turn(&mut s, "one");
    turn(&mut s, "two");
    s.mark_branch("before").unwrap();
    s.set_messages(vec![msg(
        Role::User,
        MessageContent::text("[Compacted earlier]\nsummary"),
    )]);
    let rows = s.tree_rows();
    assert_eq!(
        labels(&rows),
        ["↳ ● [Compacted earlier]", "↳ ○ one", "  ○ two  [before]"]
    );
    assert!(rows.iter().filter(|r| r.fork_start).all(|r| r.depth == 1));
}

#[test]
fn labels_use_the_first_text_line_and_are_capped() {
    let mut s = session();
    // A reply before any prompt belongs to the first turn.
    s.messages
        .push(msg(Role::Assistant, MessageContent::text("hello")));
    s.add_user_message("\n\n  first line  \nsecond");
    s.add_user_message(&"x".repeat(100));
    s.add_user_message_blocks(vec![ContentBlock::Image {
        source: ImageSource::Base64 {
            media_type: "image/png".into(),
            data: "AA==".into(),
        },
    }]);
    let rows = s.tree_rows();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].label, "first line");
    assert_eq!(rows[1].label.chars().count(), LABEL_MAX_CHARS);
    assert!(rows[1].label.ends_with('…'));
    assert_eq!(rows[2].label, "(no text)");

    let only_reply = vec![msg(Role::Assistant, MessageContent::text("hi"))];
    assert_eq!(turn_label(&only_reply), "(no text)");
    assert_eq!(turns(&only_reply), [(0, 1)]);
    assert!(turns(&[]).is_empty());
}

#[test]
fn every_block_kind_feeds_the_fingerprint() {
    let block_msg = |b: ContentBlock| msg(Role::Assistant, MessageContent::Blocks(vec![b]));
    let blocks = [
        ContentBlock::Text { text: "t".into() },
        ContentBlock::Thinking {
            text: "t".into(),
            signature: None,
        },
        ContentBlock::ToolUse {
            id: "i".into(),
            name: "n".into(),
            input: serde_json::Value::Null,
        },
        ContentBlock::ToolResult {
            tool_use_id: "i".into(),
            content: "body".into(),
            is_error: None,
        },
        ContentBlock::RedactedThinking { data: "d".into() },
        ContentBlock::Image {
            source: ImageSource::Base64 {
                media_type: "image/png".into(),
                data: "AA==".into(),
            },
        },
    ];
    let prints: Vec<u64> = blocks
        .into_iter()
        .map(|b| fingerprint(&block_msg(b)))
        .collect();
    let unique: std::collections::BTreeSet<_> = prints.iter().collect();
    assert_eq!(unique.len(), prints.len());

    // Result bodies do not count; their ids do.
    let result = |body: &str| ContentBlock::ToolResult {
        tool_use_id: "i".into(),
        content: body.into(),
        is_error: None,
    };
    assert_eq!(
        fingerprint(&block_msg(result("a"))),
        fingerprint(&block_msg(result("b")))
    );
}

#[test]
fn branches_survive_a_save_and_load() {
    let db = whycodes_storage::db::Database::open_in_memory().unwrap();
    let mut s = session();
    turn(&mut s, "one");
    s.mark_branch("mark").unwrap();
    turn(&mut s, "two");
    let one = row(&s.tree_rows(), "one").target;
    s.switch_branch(one).unwrap();
    turn(&mut s, "alt");
    s.save_to_db(&db).unwrap();

    let loaded = Session::load_from_db(&db, &s.id).unwrap().unwrap();
    assert_eq!(active_texts(&loaded), ["one", "alt"]);
    assert_eq!(loaded.branches.len(), 2);
    assert_eq!(loaded.branches[0].name.as_deref(), Some("mark"));
    assert_eq!(labels(&loaded.tree_rows()), labels(&s.tree_rows()));

    // Saving with fewer branches replaces the stored set.
    let mut fewer = loaded.clone();
    fewer.branches.truncate(1);
    fewer.save_to_db(&db).unwrap();
    let again = Session::load_from_db(&db, &s.id).unwrap().unwrap();
    assert_eq!(again.branches.len(), 1);
}

#[test]
fn a_session_saved_before_branches_loads_as_one_path() {
    let db = whycodes_storage::db::Database::open_in_memory().unwrap();
    db.create_session("old", "Old", "/tmp/tree").unwrap();
    let user = serde_json::to_string(&msg(Role::User, MessageContent::text("hi"))).unwrap();
    db.insert_message("m1", "old", "user", &user, None, None)
        .unwrap();
    let loaded = Session::load_from_db(&db, "old").unwrap().unwrap();
    assert!(loaded.branches.is_empty());
    assert_eq!(labels(&loaded.tree_rows()), ["● hi"]);
}

#[test]
fn a_corrupt_branch_row_fails_the_load() {
    let db = whycodes_storage::db::Database::open_in_memory().unwrap();
    let s = session();
    s.save_to_db(&db).unwrap();
    db.replace_transcript(&s.id, &[], &[(None, vec!["not json".into()])])
        .unwrap();
    assert!(Session::load_from_db(&db, &s.id).is_err());
}

#[test]
fn branches_round_trip_through_session_json() {
    let mut s = session();
    let bare = serde_json::to_value(&s).unwrap();
    assert!(bare.get("branches").is_none());
    let back: Session = serde_json::from_value(bare).unwrap();
    assert!(back.branches.is_empty());

    turn(&mut s, "one");
    s.mark_branch("m").unwrap();
    let back: Session = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(back.branches.len(), 1);
    assert_eq!(back.branches[0].name.as_deref(), Some("m"));
}

#[test]
fn fork_copies_the_branches() {
    let mut s = session();
    turn(&mut s, "one");
    s.mark_branch("m").unwrap();
    assert_eq!(s.fork().branches.len(), 1);
}

#[test]
fn compaction_and_undo_leave_stored_branches_alone() {
    let mut s = session();
    for i in 0..8 {
        turn(&mut s, &format!("turn {i} {}", "w".repeat(400)));
    }
    s.mark_branch("all").unwrap();
    let before = serde_json::to_string(&s.branches).unwrap();
    s.compact(50);
    s.undo_last_turn();
    s.compact_full_replace_local();
    assert_eq!(serde_json::to_string(&s.branches).unwrap(), before);
    assert_eq!(s.branches[0].messages.len(), 16);
}
