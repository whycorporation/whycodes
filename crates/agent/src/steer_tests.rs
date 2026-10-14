use super::*;

#[test]
fn clones_share_one_queue_and_drain_empties_it() {
    let inbox = SteerInbox::default();
    let ui = inbox.clone();
    ui.push("first".into());
    ui.push("second".into());
    assert_eq!(inbox.drain(), vec!["first", "second"]);
    assert!(ui.drain().is_empty());
}
