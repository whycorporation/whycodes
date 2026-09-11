use super::*;

#[test]
fn detail_omits_time_when_timestamp_is_missing() {
    let entry = crate::app::SessionEntry {
        id: "abcdef12-9999".into(),
        title: "Fix webhook".into(),
        messages: 12,
        updated_at: None,
        live: None,
    };
    assert_eq!(session_list_detail(&entry), "12 messages");
}

#[test]
fn detail_puts_time_next_to_the_message_count() {
    let entry = crate::app::SessionEntry {
        id: "abcdef12-9999".into(),
        title: "Fix webhook".into(),
        messages: 12,
        updated_at: Some(chrono::Utc::now()),
        live: None,
    };
    let detail = session_list_detail(&entry);
    assert!(
        detail.ends_with(" · 12 messages"),
        "detail {detail:?} should keep the count after the time"
    );
    assert!(
        detail.contains("just now") || detail.contains("m ago") || detail.contains("h ago"),
        "detail {detail:?} should use a Grok-style relative clock"
    );
}
