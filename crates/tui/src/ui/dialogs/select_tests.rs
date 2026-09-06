use super::*;

#[test]
fn an_item_can_carry_a_detail() {
    let plain = SelectItem::new("a");
    assert_eq!(plain.label, "a");
    assert!(plain.detail.is_none());

    let detailed = SelectItem::with_detail("a", "b");
    assert_eq!(detailed.detail.as_deref(), Some("b"));
}
