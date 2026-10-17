//! Stream rules: regexes matched against a step's streamed reply text,
//! thinking, or tool-call arguments. A hit aborts the draft and injects the
//! rule's hint (`turn.rs`).

use whycodes_config::{StreamRuleConfig, StreamRuleRepeat, StreamRuleScope};
use whycodes_session::StreamRuleLedger;

/// The streamed channel being checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamChannel<'a> {
    /// Accumulated assistant reply text of the step.
    Text,
    /// Accumulated thinking text of the step.
    Thinking,
    /// JSON arguments of one call to the named tool, as streamed so far.
    ToolArgs(&'a str),
}

/// One compiled rule.
#[derive(Debug, Clone)]
pub struct StreamRule {
    pub name: String,
    pub re: regex::Regex,
    pub hint: String,
    pub scopes: Vec<StreamRuleScope>,
    /// `tool_args` only: watched tool names. Empty = every tool.
    pub tools: Vec<String>,
    pub repeat: StreamRuleRepeat,
}

impl StreamRule {
    /// Whether this rule looks at `channel`.
    pub fn watches(&self, channel: StreamChannel<'_>) -> bool {
        match channel {
            StreamChannel::Text => self.scopes.contains(&StreamRuleScope::Text),
            StreamChannel::Thinking => self.scopes.contains(&StreamRuleScope::Thinking),
            StreamChannel::ToolArgs(tool) => {
                self.scopes.contains(&StreamRuleScope::ToolArgs)
                    && (self.tools.is_empty() || self.tools.iter().any(|t| t == tool))
            }
        }
    }
}

/// Compile config rules. Rules missing a name, pattern or hint, and invalid
/// regexes (logged), are dropped.
pub fn compile(rules: &[StreamRuleConfig]) -> Vec<StreamRule> {
    let mut out = Vec::new();
    for rule in rules {
        let name = rule.name.trim();
        let pattern = rule.pattern.trim();
        let hint = rule.hint.trim();
        if name.is_empty() || pattern.is_empty() || hint.is_empty() {
            continue;
        }
        match regex::Regex::new(pattern) {
            Ok(re) => out.push(StreamRule {
                name: name.to_string(),
                re,
                hint: hint.to_string(),
                scopes: rule.scopes(),
                tools: rule.tools.clone(),
                repeat: rule.repeat,
            }),
            Err(e) => {
                tracing::warn!(name, pattern, error = %e, "invalid stream rule regex");
            }
        }
    }
    out
}

/// Every rule that watches `channel` and matches `text`, ignoring `repeat`.
pub fn matching<'a>(
    rules: &'a [StreamRule],
    channel: StreamChannel<'_>,
    text: &str,
) -> Vec<&'a StreamRule> {
    rules
        .iter()
        .filter(|r| r.watches(channel) && r.re.is_match(text))
        .collect()
}

/// First rule that watches `channel`, may still fire under its `repeat`
/// policy, and matches `text`.
pub fn first_hit<'a>(
    rules: &'a [StreamRule],
    channel: StreamChannel<'_>,
    text: &str,
    ledger: &StreamRuleLedger,
) -> Option<&'a StreamRule> {
    rules.iter().find(|r| {
        r.watches(channel)
            && r.repeat.allows(ledger.last_fired(&r.name), ledger.turn)
            && r.re.is_match(text)
    })
}

#[cfg(test)]
#[path = "stream_rules_tests.rs"]
mod tests;
