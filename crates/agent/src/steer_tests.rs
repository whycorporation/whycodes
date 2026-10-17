use super::*;
use whycodes_core::types::ContentBlock;

fn text(s: &str) -> MessageContent {
    MessageContent::Text(s.into())
}

#[test]
fn clones_share_queues_kept_apart_by_session() {
    let inbox = SteerInbox::default();
    let ui = inbox.clone();
    ui.push("a", text("first"));
    ui.push("b", text("other session"));
    ui.push("a", text("second"));
    let got: Vec<String> = inbox.drain("a").iter().map(content_text).collect();
    assert_eq!(got, ["first", "second"]);
    assert!(ui.drain("a").is_empty());
    assert_eq!(inbox.drain("b").len(), 1);
}

#[test]
fn content_text_keeps_text_blocks_only() {
    let blocks = MessageContent::Blocks(vec![
        ContentBlock::Text {
            text: "look".into(),
        },
        ContentBlock::Image {
            source: whycodes_core::types::ImageSource::Url {
                url: "https://example.invalid/a.png".into(),
            },
        },
        ContentBlock::Text {
            text: "here".into(),
        },
    ]);
    assert_eq!(content_text(&blocks), "look\nhere");
    assert_eq!(content_text(&text("plain")), "plain");
}

#[test]
fn the_turn_reads_next_step_and_urgent_but_leaves_queued() {
    let inbox = SteerInbox::default();
    assert!(inbox.take_for_turn("a").is_empty());
    assert!(!inbox.has_urgent("a"));
    inbox.push_mode("a", SteerMode::Queue, text("later"));
    inbox.push("a", text("soon"));
    assert!(!inbox.has_urgent("a"));
    inbox.push_mode("a", SteerMode::Urgent, text("now"));
    assert!(inbox.has_urgent("a"));
    let read: Vec<String> = inbox.take_for_turn("a").iter().map(content_text).collect();
    assert_eq!(read, ["soon", "now"]);
    assert!(!inbox.has_urgent("a"));
    let left: Vec<String> = inbox.drain("a").iter().map(content_text).collect();
    assert_eq!(left, ["later"]);
}

#[test]
fn mode_names_on_the_wire() {
    let mode: SteerMode = serde_json::from_str("\"urgent\"").unwrap();
    assert_eq!(mode, SteerMode::Urgent);
    assert_eq!(
        serde_json::to_string(&SteerMode::NextStep).unwrap(),
        "\"next_step\""
    );
    assert_eq!(SteerMode::default(), SteerMode::NextStep);
}
