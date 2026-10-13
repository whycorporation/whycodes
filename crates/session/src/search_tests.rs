use super::*;
use crate::session::Session;

fn db() -> Database {
    Database::open(":memory:").unwrap()
}

fn saved(db: &Database, title: &str, user: &str, reply: &str) -> Session {
    let mut s = Session::new("/p".into(), "sys".into());
    s.set_title_manual(title);
    s.add_user_message(user);
    s.add_assistant_message(vec![ContentBlock::Text { text: reply.into() }]);
    s.save_to_db(db).unwrap();
    s
}

#[test]
fn query_terms_dedupe_and_cap() {
    assert_eq!(query_terms("  retry  Retry loop "), ["retry", "loop"]);
    assert!(query_terms("   ").is_empty());
    assert_eq!(query_terms("a b c d e f g h i j").len(), MAX_TERMS);
}

#[test]
fn search_needs_every_word_in_one_message() {
    let db = db();
    let retry = saved(&db, "retry", "explain the Retry loop", "it backs off");
    saved(&db, "other", "retry", "loop is elsewhere");
    let hits = search_sessions(&db, "retry LOOP", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, retry.id);
    assert_eq!(hits[0].title, "retry");
    assert_eq!(hits[0].hits, 1);
    assert_eq!(hits[0].snippet, "explain the Retry loop");
    assert!(search_sessions(&db, "", 10).unwrap().is_empty());
    assert!(search_sessions(&db, "retry", 0).unwrap().is_empty());
    // LIKE wildcards in the query are literal.
    assert!(search_sessions(&db, "100%", 10).unwrap().is_empty());
    assert!(search_sessions(&db, "re_ry", 10).unwrap().is_empty());
}

#[test]
fn snippets_center_on_the_term_and_read_blocks() {
    let long = format!("{} needle {}", "a ".repeat(100), "b ".repeat(100));
    let msg = serde_json::to_string(&Message {
        role: whycodes_core::types::Role::User,
        content: MessageContent::Blocks(vec![
            ContentBlock::Text { text: long },
            ContentBlock::ToolUse {
                id: "t".into(),
                name: "read".into(),
                input: serde_json::json!({"path": "x"}),
            },
            ContentBlock::ToolResult {
                tool_use_id: "t".into(),
                content: "file body".into(),
                is_error: None,
            },
            ContentBlock::Image {
                source: whycodes_core::types::ImageSource::Base64 {
                    media_type: "image/png".into(),
                    data: "AA".into(),
                },
            },
        ]),
        tool_call_id: None,
        name: None,
        created_at: None,
    })
    .unwrap();
    let s = snippet(&msg, &["NEEDLE".into()], 30);
    assert!(s.starts_with('…') && s.ends_with('…'), "{s}");
    assert!(s.contains("needle"), "{s}");
    assert!(snippet(&msg, &["body".into()], 1000).ends_with("file body"));
    assert_eq!(snippet("not json", &["zzz".into()], 100), "not json");
    assert_eq!(snippet("", &[], 10), "");
}
