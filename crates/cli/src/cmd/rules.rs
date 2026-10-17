//! `whycodes rules` — list stream rules and test which would fire.

use super::helpers::*;
use crate::Cli;
use crate::args::RulesCmd;
use whycodes_agent::stream_rules::{self, StreamChannel};
use whycodes_config::{Config, StreamRuleConfig, StreamRuleScope};

pub(crate) fn cmd_rules(cli: &Cli, cmd: &RulesCmd) -> anyhow::Result<()> {
    let project_dir = resolve_dir(cli);
    let config = Config::load_layered(&project_dir)?;
    let file_rules = whycodes_config::stream_rule_files(&project_dir);
    let out = match cmd {
        RulesCmd::List => list_report(&config.session.stream_rules, &file_rules),
        RulesCmd::Test { text, scope, tool } => {
            test_report(&config.session.stream_rules, text, scope, tool.as_deref())
        }
    };
    print!("{out}");
    Ok(())
}

/// One block per rule: name, source, scope, tools, repeat, pattern, hint.
pub(crate) fn list_report(rules: &[StreamRuleConfig], file_rules: &[StreamRuleConfig]) -> String {
    if rules.is_empty() {
        return "No stream rules. Add [[session.stream_rules]] or .whycodes/rules/*.md with a `pattern` frontmatter key.\n".into();
    }
    let active = stream_rules::compile(rules);
    let mut out = String::new();
    for rule in rules {
        let name = rule.name.trim();
        let source = if file_rules.iter().any(|f| f.name == name) {
            "file"
        } else {
            "config"
        };
        let state = if active.iter().any(|a| a.name == name) {
            ""
        } else {
            "  (inactive: missing name/pattern/hint or invalid regex)"
        };
        out.push_str(&format!(
            "{name}  [{source}]  scope={}  repeat={}{state}\n",
            scope_label(&rule.scopes()),
            rule.repeat.label()
        ));
        if !rule.tools.is_empty() {
            out.push_str(&format!("  tools: {}\n", rule.tools.join(", ")));
        }
        out.push_str(&format!("  pattern: {}\n", rule.pattern));
        out.push_str(&format!("  hint: {}\n", first_line(&rule.hint)));
    }
    out
}

/// Which rules match `text` on each requested channel (default `text`).
pub(crate) fn test_report(
    rules: &[StreamRuleConfig],
    text: &str,
    scopes: &[StreamRuleScope],
    tool: Option<&str>,
) -> String {
    let compiled = stream_rules::compile(rules);
    let scopes = if scopes.is_empty() {
        &[StreamRuleScope::Text][..]
    } else {
        scopes
    };
    let mut out = String::new();
    for &scope in scopes {
        let channel = match scope {
            StreamRuleScope::Text => StreamChannel::Text,
            StreamRuleScope::Thinking => StreamChannel::Thinking,
            StreamRuleScope::ToolArgs => StreamChannel::ToolArgs(tool.unwrap_or("")),
        };
        for rule in stream_rules::matching(&compiled, channel, text) {
            out.push_str(&format!(
                "{}: {}  (repeat {})\n  {}\n",
                scope.as_str(),
                rule.name,
                rule.repeat.label(),
                first_line(&rule.hint)
            ));
        }
    }
    if out.is_empty() {
        out.push_str("No stream rule would fire.\n");
    }
    out
}

fn scope_label(scopes: &[StreamRuleScope]) -> String {
    scopes
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

fn first_line(text: &str) -> &str {
    text.trim().lines().next().unwrap_or("")
}

#[cfg(test)]
#[path = "rules_tests.rs"]
mod tests;
