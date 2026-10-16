use super::*;
use crate::cmd::helpers::IsolatedHome;
use crate::{Cli, Commands};
use clap::Parser;
use whycodes_config::StreamRuleRepeat;

fn rule(name: &str, pattern: &str) -> StreamRuleConfig {
    StreamRuleConfig {
        name: name.into(),
        pattern: pattern.into(),
        hint: format!("hint {name}\nsecond line"),
        ..Default::default()
    }
}

#[test]
fn list_marks_source_scope_and_inactive_rules() {
    assert!(list_report(&[], &[]).starts_with("No stream rules."));
    let file = StreamRuleConfig {
        scope: vec![StreamRuleScope::Thinking, StreamRuleScope::ToolArgs],
        tools: vec!["bash".into(), "edit".into()],
        repeat: StreamRuleRepeat::AfterTurns(2),
        ..rule("from-file", "rm")
    };
    let rules = [rule("table", "TODO"), rule("broken", "("), file.clone()];
    let out = list_report(&rules, &[file]);
    assert!(
        out.contains("table  [config]  scope=text  repeat=always\n"),
        "{out}"
    );
    assert!(out.contains("broken  [config]  scope=text  repeat=always  (inactive"));
    assert!(out.contains("from-file  [file]  scope=thinking,tool_args  repeat=after_turns=2\n"));
    assert!(out.contains("  tools: bash, edit\n"));
    assert!(out.contains("  pattern: TODO\n"));
    assert!(out.contains("  hint: hint table\n"));
    assert!(!out.contains("second line"));
}

#[test]
fn test_report_per_channel() {
    let tool_rule = StreamRuleConfig {
        scope: vec![StreamRuleScope::ToolArgs],
        tools: vec!["bash".into()],
        ..rule("no-rm", "rm -rf")
    };
    let think = StreamRuleConfig {
        scope: vec![StreamRuleScope::Thinking],
        ..rule("think", "rm")
    };
    let rules = [rule("text-rm", "rm"), tool_rule, think];
    assert_eq!(
        test_report(&rules, "rm -rf /", &[], None),
        "text: text-rm  (repeat always)\n  hint text-rm\n"
    );
    let all = [
        StreamRuleScope::Text,
        StreamRuleScope::Thinking,
        StreamRuleScope::ToolArgs,
    ];
    let out = test_report(&rules, "rm -rf /", &all, Some("bash"));
    assert!(out.contains("thinking: think"), "{out}");
    assert!(out.contains("tool_args: no-rm"), "{out}");
    let out = test_report(&rules, "rm -rf /", &[StreamRuleScope::ToolArgs], None);
    assert_eq!(out, "No stream rule would fire.\n");
}

#[test]
fn rules_command_parses_and_runs() {
    let _home = IsolatedHome::new();
    let dir = tempfile::tempdir().unwrap();
    let rules_dir = whycodes_core::project_dir(dir.path()).join("rules");
    std::fs::create_dir_all(&rules_dir).unwrap();
    std::fs::write(
        rules_dir.join("no-todo.md"),
        "---\npattern: TODO\n---\nFinish the work.\n",
    )
    .unwrap();
    let path = dir.path().display().to_string();
    let cli = Cli::try_parse_from(["whycodes", "rules", "list", "-d", &path]).unwrap();
    match &cli.command {
        Some(Commands::Rules { cmd }) => cmd_rules(&cli, cmd).unwrap(),
        other => panic!("parsed {other:?}"),
    }
    let cli = Cli::try_parse_from([
        "whycodes",
        "rules",
        "test",
        "a TODO here",
        "--scope",
        "text,thinking",
        "--tool",
        "bash",
        "-d",
        &path,
    ])
    .unwrap();
    match &cli.command {
        Some(Commands::Rules {
            cmd: cmd @ RulesCmd::Test { text, scope, tool },
        }) => {
            assert_eq!(text, "a TODO here");
            assert_eq!(scope, &[StreamRuleScope::Text, StreamRuleScope::Thinking]);
            assert_eq!(tool.as_deref(), Some("bash"));
            cmd_rules(&cli, cmd).unwrap();
        }
        other => panic!("parsed {other:?}"),
    }
    let err = Cli::try_parse_from(["whycodes", "rules", "test", "x", "--scope", "nope"])
        .unwrap_err()
        .to_string();
    assert!(err.contains("unknown scope `nope`"), "{err}");
}
