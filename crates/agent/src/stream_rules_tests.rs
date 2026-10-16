use super::*;

fn cfg(name: &str, pattern: &str) -> StreamRuleConfig {
    StreamRuleConfig {
        name: name.into(),
        pattern: pattern.into(),
        hint: "h".into(),
        ..Default::default()
    }
}

#[test]
fn compile_drops_incomplete_and_invalid() {
    let rules = [
        cfg("", "x"),
        cfg("bad", "("),
        StreamRuleConfig {
            hint: " ".into(),
            ..cfg("nohint", "x")
        },
        StreamRuleConfig {
            hint: " use Arc ".into(),
            ..cfg(" no-leak ", "Box::leak")
        },
    ];
    let compiled = compile(&rules);
    assert_eq!(compiled.len(), 1);
    assert_eq!(compiled[0].name, "no-leak");
    assert_eq!(compiled[0].hint, "use Arc");
    assert_eq!(compiled[0].scopes, vec![StreamRuleScope::Text]);
    let ledger = StreamRuleLedger::default();
    let hit = first_hit(
        &compiled,
        StreamChannel::Text,
        "please Box::leak this",
        &ledger,
    );
    assert_eq!(hit.map(|r| r.name.as_str()), Some("no-leak"));
    assert!(first_hit(&compiled, StreamChannel::Text, "Arc::from", &ledger).is_none());
    assert!(first_hit(&compiled, StreamChannel::Thinking, "Box::leak", &ledger).is_none());
}

#[test]
fn channels_and_tool_filter() {
    let rules = compile(&[
        StreamRuleConfig {
            scope: vec![StreamRuleScope::Thinking, StreamRuleScope::ToolArgs],
            tools: vec!["bash".into()],
            ..cfg("rm", "rm -rf")
        },
        StreamRuleConfig {
            scope: vec![StreamRuleScope::ToolArgs],
            ..cfg("any-tool", "rm")
        },
    ]);
    let text = "rm -rf /";
    let names = |hits: Vec<&StreamRule>| hits.iter().map(|r| r.name.clone()).collect::<Vec<_>>();
    assert!(matching(&rules, StreamChannel::Text, text).is_empty());
    assert_eq!(
        names(matching(&rules, StreamChannel::Thinking, text)),
        ["rm"]
    );
    assert_eq!(
        names(matching(&rules, StreamChannel::ToolArgs("bash"), text)),
        ["rm", "any-tool"]
    );
    assert_eq!(
        names(matching(&rules, StreamChannel::ToolArgs("edit"), text)),
        ["any-tool"]
    );
}

#[test]
fn repeat_policy_gates_hits() {
    let rules = compile(&[
        cfg("once", "x"),
        StreamRuleConfig {
            repeat: StreamRuleRepeat::AfterTurns(2),
            ..cfg("later", "x")
        },
        StreamRuleConfig {
            repeat: StreamRuleRepeat::Always,
            ..cfg("always", "x")
        },
    ]);
    let mut ledger = StreamRuleLedger::default();
    ledger.begin_turn();
    let hit = |l: &StreamRuleLedger| {
        first_hit(&rules, StreamChannel::Text, "x", l).map(|r| r.name.clone())
    };
    assert_eq!(hit(&ledger).as_deref(), Some("once"));
    ledger.record("once");
    assert_eq!(hit(&ledger).as_deref(), Some("later"));
    ledger.record("later");
    assert_eq!(hit(&ledger).as_deref(), Some("always"));
    ledger.begin_turn();
    ledger.begin_turn();
    assert_eq!(hit(&ledger).as_deref(), Some("later"));
}
