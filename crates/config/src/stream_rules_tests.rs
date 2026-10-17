use super::*;
use crate::types::{StreamRuleConfig, StreamRuleRepeat, StreamRuleScope};

fn write_rule(dir: &Path, file: &str, body: &str) {
    let rules = whycodes_core::project_dir(dir).join("rules");
    std::fs::create_dir_all(&rules).unwrap();
    std::fs::write(rules.join(file), body).unwrap();
}

#[test]
fn parse_full_frontmatter() {
    let rule = parse_rule_markdown(
        "stem",
        "\u{feff}---\nname: no-rm\npattern: 'rm -rf'\nscope: [tool_args, \"thinking\", bogus]\ntools: [bash, edit]\nrepeat: { after_turns: 3 }\nignored: x\nnot a key\n---\nDo not delete trees.\n",
    )
    .unwrap();
    assert_eq!(rule.name, "no-rm");
    assert_eq!(rule.pattern, "rm -rf");
    assert_eq!(
        rule.scope,
        vec![StreamRuleScope::ToolArgs, StreamRuleScope::Thinking]
    );
    assert_eq!(rule.tools, vec!["bash", "edit"]);
    assert_eq!(rule.repeat, StreamRuleRepeat::AfterTurns(3));
    assert_eq!(rule.hint, "Do not delete trees.");
}

#[test]
fn parse_block_lists_and_defaults() {
    let rule = parse_rule_markdown(
        "todo",
        "---
pattern: \\bTODO\\b
scope:
  - text
  - thinking
tools:
  - bash
repeat: sometimes
---
",
    )
    .unwrap();
    assert_eq!(rule.name, "todo");
    assert_eq!(rule.pattern, r"\bTODO\b");
    assert_eq!(
        rule.scope,
        vec![StreamRuleScope::Text, StreamRuleScope::Thinking]
    );
    assert_eq!(rule.tools, vec!["bash"]);
    assert_eq!(rule.repeat, StreamRuleRepeat::Always);
    assert_eq!(rule.hint, "");
}

#[test]
fn parse_rejects_files_without_pattern() {
    assert!(parse_rule_markdown("a", "plain context rule").is_none());
    assert!(parse_rule_markdown("a", "---\nname: x\n---\nbody").is_none());
    assert!(parse_rule_markdown("a", "---\npattern: x\nno close").is_none());
    assert!(parse_rule_markdown("a", "---\npattern:\n---\nbody").is_none());
}

#[test]
fn unquote_forms() {
    assert_eq!(unquote("'it''s'"), "it's");
    assert_eq!(unquote(r#""a\"b\\c\d\""#), r#"a"b\c\d\"#);
    assert_eq!(unquote(r#""tail\"#), r#""tail\"#);
    assert_eq!(unquote(r#""x\""#), r#"x\"#);
    assert_eq!(unquote(" plain "), "plain");
    assert_eq!(list("[a, , 'b']"), vec!["a", "b"]);
    assert_eq!(list("solo"), vec!["solo"]);
}

#[test]
fn files_merge_over_config_rules() {
    let dir = tempfile::tempdir().unwrap();
    write_rule(
        dir.path(),
        "b.md",
        "---\nname: shared\npattern: file\n---\nfrom file",
    );
    write_rule(dir.path(), "a.md", "---\npattern: new\n---\nfresh");
    write_rule(dir.path(), "plain.md", "just context");
    write_rule(dir.path(), "skip.txt", "---\npattern: x\n---\nno");
    std::fs::create_dir_all(whycodes_core::project_dir(dir.path()).join("rules/sub.md")).unwrap();
    let mut cfg = Config::default();
    cfg.session.stream_rules = vec![
        StreamRuleConfig {
            name: " shared ".into(),
            pattern: "table".into(),
            hint: "from table".into(),
            ..Default::default()
        },
        StreamRuleConfig {
            name: "keep".into(),
            pattern: "k".into(),
            hint: "kept".into(),
            ..Default::default()
        },
    ];
    cfg.load_stream_rule_files(dir.path());
    let names: Vec<_> = cfg
        .session
        .stream_rules
        .iter()
        .map(|r| (r.name.as_str(), r.hint.as_str()))
        .collect();
    assert_eq!(
        names,
        vec![("shared", "from file"), ("keep", "kept"), ("a", "fresh")]
    );
}

#[test]
fn missing_dir_and_unreadable_file() {
    let dir = tempfile::tempdir().unwrap();
    assert!(stream_rule_files(dir.path()).is_empty());
    assert!(read_rule_file(&dir.path().join("gone.md")).is_none());
}

#[test]
fn scope_and_repeat_helpers() {
    let r = StreamRuleConfig::default();
    assert_eq!(r.scopes(), vec![StreamRuleScope::Text]);
    let r = StreamRuleConfig {
        scope: vec![StreamRuleScope::ToolArgs],
        ..Default::default()
    };
    assert_eq!(r.scopes(), vec![StreamRuleScope::ToolArgs]);
    assert_eq!(
        StreamRuleScope::parse("Tool-Args"),
        Some(StreamRuleScope::ToolArgs)
    );
    assert_eq!(StreamRuleScope::parse("nope"), None);
    for s in [
        StreamRuleScope::Text,
        StreamRuleScope::Thinking,
        StreamRuleScope::ToolArgs,
    ] {
        assert_eq!(StreamRuleScope::parse(s.as_str()), Some(s));
    }
    assert_eq!(
        StreamRuleRepeat::parse("ONCE"),
        Some(StreamRuleRepeat::Once)
    );
    assert_eq!(
        StreamRuleRepeat::parse("always"),
        Some(StreamRuleRepeat::Always)
    );
    assert_eq!(
        StreamRuleRepeat::parse("after_turns = 2"),
        Some(StreamRuleRepeat::AfterTurns(2))
    );
    assert_eq!(
        StreamRuleRepeat::parse("4"),
        Some(StreamRuleRepeat::AfterTurns(4))
    );
    assert_eq!(StreamRuleRepeat::parse("every: 2"), None);
    assert_eq!(StreamRuleRepeat::parse("x"), None);
    assert!(StreamRuleRepeat::Once.allows(None, 0));
    assert!(!StreamRuleRepeat::Once.allows(Some(1), 9));
    assert!(StreamRuleRepeat::Always.allows(Some(1), 1));
    assert!(!StreamRuleRepeat::AfterTurns(2).allows(Some(1), 2));
    assert!(StreamRuleRepeat::AfterTurns(2).allows(Some(1), 3));
    assert_eq!(StreamRuleRepeat::Once.label(), "once");
    assert_eq!(StreamRuleRepeat::Always.label(), "always");
    assert_eq!(StreamRuleRepeat::AfterTurns(5).label(), "after_turns=5");
}

#[test]
fn toml_forms_and_old_configs_round_trip() {
    #[derive(serde::Deserialize, serde::Serialize)]
    struct Wrap {
        rule: Vec<StreamRuleConfig>,
    }
    let old = "[[rule]]\nname = \"a\"\npattern = \"x\"\nhint = \"h\"\n";
    let w: Wrap = toml::from_str(old).unwrap();
    assert_eq!(w.rule[0].repeat, StreamRuleRepeat::Always);
    assert!(w.rule[0].scope.is_empty());
    assert_eq!(toml::to_string(&w).unwrap(), old);
    let new = "[[rule]]\nname = \"a\"\npattern = \"x\"\nhint = \"h\"\nscope = [\"thinking\", \"tool_args\"]\ntools = [\"bash\"]\nrepeat = \"once\"\n\n[[rule]]\nname = \"b\"\npattern = \"y\"\nhint = \"h\"\n\n[rule.repeat]\nafter_turns = 2\n";
    let w: Wrap = toml::from_str(new).unwrap();
    assert_eq!(w.rule[0].repeat, StreamRuleRepeat::Once);
    assert_eq!(
        w.rule[0].scope,
        vec![StreamRuleScope::Thinking, StreamRuleScope::ToolArgs]
    );
    assert_eq!(w.rule[1].repeat, StreamRuleRepeat::AfterTurns(2));
    let inline: Wrap = toml::from_str(
        "[[rule]]\nname = \"b\"\npattern = \"y\"\nhint = \"h\"\nrepeat = { after_turns = 3 }\n",
    )
    .unwrap();
    assert_eq!(inline.rule[0].repeat, StreamRuleRepeat::AfterTurns(3));
}
