use super::*;

#[test]
fn builders_match_json_macro() {
    assert_eq!(
        obj([("type", str("ephemeral"))]),
        serde_json::json!({ "type": "ephemeral" })
    );
    assert_eq!(bool(true), Value::Bool(true));
    assert_eq!(arr([str("a"), str("b")]), serde_json::json!(["a", "b"]));
    let mut v = obj([("k", str("v"))]);
    insert(&mut v, "n", Value::from(1u64));
    assert_eq!(v["n"], 1);
    insert(&mut Value::Null, "k", str("x"));
    let map = map_from([("a", str("b"))]);
    assert_eq!(map["a"], "b");
}
