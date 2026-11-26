//! The YAML subset skill and agent files use in their `---` frontmatter:
//! `key: value`, quoted values, inline `[a, b]` lists, `- item` lists, and
//! `|` / `>` block text. Deliberately not a full YAML parser.
//!
//! One parser for both: two copies had drifted, and the skill one kept the
//! quotes around `name: "my-skill"` and read `description: >` as `>`.

use std::collections::HashMap;

/// One frontmatter value.
#[derive(Debug, Clone, PartialEq)]
pub enum Field {
    Text(String),
    List(Vec<String>),
}

impl Field {
    /// The value as text; a list joins with `, `.
    pub fn text(&self) -> String {
        match self {
            Field::Text(s) => s.clone(),
            Field::List(items) => items.join(", "),
        }
    }

    /// The value as items; text splits on `,`.
    pub fn items(&self) -> Vec<String> {
        match self {
            Field::Text(s) => s.split(',').map(|t| t.trim().to_string()).collect(),
            Field::List(items) => items.clone(),
        }
    }
}

/// Parse a frontmatter block (the text between the `---` markers).
pub fn parse(block: &str) -> HashMap<String, Field> {
    let mut map: HashMap<String, Field> = HashMap::new();
    let mut key: Option<String> = None;
    let mut fold = " ";
    for line in block.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if let (true, Some(k)) = (indented, &key) {
            let entry = map.entry(k.clone()).or_insert(Field::Text(String::new()));
            match (trimmed.strip_prefix("- "), entry) {
                (Some(item), Field::List(items)) => items.push(unquote(item)),
                (Some(item), slot @ Field::Text(_)) if slot.text().is_empty() => {
                    *slot = Field::List(vec![unquote(item)]);
                }
                (_, Field::Text(s)) => {
                    if !s.is_empty() {
                        s.push_str(fold);
                    }
                    s.push_str(trimmed);
                }
                (None, Field::List(items)) => items.push(unquote(trimmed)),
            }
            continue;
        }
        let Some((k, v)) = trimmed.split_once(':') else {
            continue;
        };
        let k = k.trim().to_string();
        let v = v.trim();
        fold = if v.starts_with('|') { "\n" } else { " " };
        let field = if v.starts_with('|') || v.starts_with('>') || v.is_empty() {
            Field::Text(String::new())
        } else if let Some(inner) = v.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            Field::List(
                inner
                    .split(',')
                    .map(unquote)
                    .filter(|s| !s.is_empty())
                    .collect(),
            )
        } else {
            Field::Text(unquote(v))
        };
        map.insert(k.clone(), field);
        key = Some(k);
    }
    map
}

/// `"x"` / `'x'` → `x`; anything else trimmed.
pub fn unquote(s: &str) -> String {
    let s = s.trim();
    let quoted = s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')));
    if quoted {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_blocks_and_lists() {
        let fields = parse(
            "name: \"my-skill\"\ndescription: >\n  Reviews code\n  for bugs: carefully\nnotes: |\n  a\n  b\ntools: [read, 'grep']\nfiles:\n  - \"*.rs\"\n  - Cargo.toml\n# c\n",
        );
        assert_eq!(fields["name"], Field::Text("my-skill".into()));
        assert_eq!(
            fields["description"].text(),
            "Reviews code for bugs: carefully"
        );
        assert_eq!(fields["notes"].text(), "a\nb");
        assert_eq!(fields["tools"].items(), vec!["read", "grep"]);
        assert_eq!(fields["files"].items(), vec!["*.rs", "Cargo.toml"]);
        assert_eq!(fields["files"].text(), "*.rs, Cargo.toml");
        assert_eq!(Field::Text("a, b".into()).items(), vec!["a", "b"]);
        assert!(!fields.contains_key("for bugs"));
    }

    #[test]
    fn stray_lines_are_skipped() {
        let fields = parse("  - orphan\nnot a pair\nlist:\n  - a\n  b\n");
        assert_eq!(fields.len(), 1);
        assert_eq!(fields["list"].items(), vec!["a", "b"]);
        assert_eq!(unquote("'"), "'");
    }
}
