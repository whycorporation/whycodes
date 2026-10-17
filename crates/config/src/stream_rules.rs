//! Stream rules from `.whycodes/rules/*.md`.
//!
//! A rule file whose YAML frontmatter names a `pattern` is a stream rule:
//! optional `name` (default: file stem), `scope`, `tools`, `repeat`; the body
//! is the hint. Files without `pattern` stay plain context rules.

use std::path::{Path, PathBuf};

use crate::types::{Config, StreamRuleConfig, StreamRuleRepeat, StreamRuleScope};

impl Config {
    /// Merge `.whycodes/rules/*.md` stream rules into `session.stream_rules`.
    /// A file rule replaces a config-table rule of the same name.
    pub fn load_stream_rule_files(&mut self, project_dir: &Path) {
        for rule in stream_rule_files(project_dir) {
            let rules = &mut self.session.stream_rules;
            match rules.iter_mut().find(|r| r.name.trim() == rule.name) {
                Some(slot) => *slot = rule,
                None => rules.push(rule),
            }
        }
    }
}

/// Stream rules defined by files in the first [`whycodes_core::project_rule_dirs`]
/// entry (`.whycodes/rules/*.md`), sorted by path.
pub fn stream_rule_files(project_dir: &Path) -> Vec<StreamRuleConfig> {
    let [(dir, ext), ..] = whycodes_core::project_rule_dirs(project_dir);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case(ext))
        })
        .collect();
    paths.sort();
    paths.iter().filter_map(|p| read_rule_file(p)).collect()
}

fn read_rule_file(path: &Path) -> Option<StreamRuleConfig> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "stream rule file unreadable");
            return None;
        }
    };
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    parse_rule_markdown(&stem, &text)
}

/// Parse one rule file. `None` when it has no frontmatter `pattern`.
pub fn parse_rule_markdown(stem: &str, content: &str) -> Option<StreamRuleConfig> {
    let rest = content
        .trim_start_matches('\u{feff}')
        .trim_start()
        .strip_prefix("---")?;
    let (front, body) = rest.split_once("\n---")?;
    let body = body.split_once('\n').map_or("", |(_, b)| b);
    let mut rule = StreamRuleConfig {
        name: stem.to_string(),
        hint: body.trim().to_string(),
        ..Default::default()
    };
    // Key of a block list (`scope:` then `- text` lines) being read.
    let mut list_key = String::new();
    for line in front.lines() {
        let trimmed = line.trim();
        if let Some(item) = trimmed.strip_prefix("- ") {
            apply_key(&mut rule, &list_key, item, true);
            continue;
        }
        let Some((k, v)) = trimmed.split_once(':') else {
            continue;
        };
        list_key = k.trim().to_string();
        apply_key(&mut rule, &list_key, v.trim(), false);
    }
    (!rule.pattern.is_empty()).then_some(rule)
}

fn apply_key(rule: &mut StreamRuleConfig, key: &str, value: &str, append: bool) {
    if value.is_empty() {
        return;
    }
    match key {
        "pattern" => rule.pattern = unquote(value),
        "name" => rule.name = unquote(value),
        "scope" => {
            if !append {
                rule.scope.clear();
            }
            for item in list(value) {
                match StreamRuleScope::parse(&item) {
                    Some(s) => rule.scope.push(s),
                    None => {
                        tracing::warn!(rule = %rule.name, scope = %item, "unknown stream rule scope")
                    }
                }
            }
        }
        "tools" => {
            if !append {
                rule.tools.clear();
            }
            rule.tools.extend(list(value));
        }
        "repeat" => match StreamRuleRepeat::parse(value) {
            Some(r) => rule.repeat = r,
            None => {
                tracing::warn!(rule = %rule.name, repeat = %value, "unknown stream rule repeat")
            }
        },
        _ => {}
    }
}

/// `[a, "b"]` or a bare `a` → items.
fn list(value: &str) -> Vec<String> {
    let inner = value.trim().trim_start_matches('[').trim_end_matches(']');
    inner
        .split(',')
        .map(unquote)
        .filter(|s| !s.is_empty())
        .collect()
}

/// YAML scalar: `'single'` (`''` = `'`), `"double"` (`\"` and `\`), or plain.
fn unquote(value: &str) -> String {
    let v = value.trim();
    if let Some(inner) = v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        return inner.replace("''", "'");
    }
    let Some(inner) = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return v.to_string();
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some(n @ ('\\' | '"')) => out.push(n),
            Some(n) => {
                out.push('\\');
                out.push(n);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
#[path = "stream_rules_tests.rs"]
mod tests;
