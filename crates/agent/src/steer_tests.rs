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
