//! Dotted keys over the config tree (`session.tool_profile`,
//! `agents.build.model`, `mcp_servers."my.server".command`).
//!
//! Read side: [`lookup`], [`leaves`], [`render`] work on the serialized
//! [`crate::Config`] (`toml::Value`), and [`redact`] hides secret values.
//! Write side: [`set_in_document`] changes one key of a TOML document and
//! keeps every other key, comment and blank line as it was.
//!
//! A segment that meets an array picks an element: a number is an index,
//! anything else the table whose `name` equals it (`agents.build`).

use toml::Value;
use toml_edit::{DocumentMut, Item, TableLike};
use whycodes_core::{Error, Result};

/// Printed in place of a secret value.
pub const REDACTED: &str = "<redacted>";

/// Last segments that hold a secret, matched whole or as a `_<word>` suffix
/// (`telegram_bot_token`, `discord_webhook`).
const SECRET_WORDS: [&str; 8] = [
    "api_key",
    "apikey",
    "token",
    "secret",
    "password",
    "webhook",
    "webhook_url",
    "authorization",
];

/// Split a dotted key into segments (TOML dotted-key syntax, so a segment
/// with a dot in it is quoted: `models."gpt-4.1"`).
pub fn parse_key(key: &str) -> Result<Vec<String>> {
    let keys = toml_edit::Key::parse(key)
        .map_err(|e| Error::Config(format!("invalid config key '{key}': {e}")))?;
    Ok(keys.iter().map(|k| k.get().to_string()).collect())
}

/// The dotted form of `path`, quoting segments that are not bare keys.
pub fn format_key(path: &[String]) -> String {
    let segs: Vec<String> = path
        .iter()
        .map(|s| toml_edit::Key::new(s.as_str()).display_repr().into_owned())
        .collect();
    segs.join(".")
}

/// The value at `path`, if any.
pub fn lookup<'a>(root: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter().try_fold(root, |node, seg| child(node, seg))
}

fn child<'a>(node: &'a Value, seg: &str) -> Option<&'a Value> {
    match node {
        Value::Table(table) => table.get(seg),
        Value::Array(items) => match seg.parse::<usize>().ok() {
            Some(index) => items.get(index),
            None => items.iter().find(|item| element_name(item) == Some(seg)),
        },
        _ => None,
    }
}

fn element_name(item: &Value) -> Option<&str> {
    item.get("name").and_then(Value::as_str)
}

/// Every leaf under `root` with its path, sorted by path. Tables recurse,
/// and so do arrays of tables (each element by its `name`, else its
/// index); scalars, other arrays and empty tables are leaves.
pub fn leaves(root: &Value) -> Vec<(Vec<String>, Value)> {
    let mut out = Vec::new();
    collect_leaves(root, &mut Vec::new(), &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn collect_leaves(node: &Value, path: &mut Vec<String>, out: &mut Vec<(Vec<String>, Value)>) {
    let children: Vec<(String, &Value)> = match node {
        Value::Table(table) => table.iter().map(|(k, v)| (k.clone(), v)).collect(),
        Value::Array(items) if items.iter().all(Value::is_table) => items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                (
                    element_name(item).map_or(i.to_string(), str::to_string),
                    item,
                )
            })
            .collect(),
        _ => Vec::new(),
    };
    if children.is_empty() {
        out.push((path.clone(), node.clone()));
        return;
    }
    for (seg, item) in children {
        path.push(seg);
        collect_leaves(item, path, out);
        path.pop();
    }
}

/// True when the key at `path` holds a secret: the last segment is (or ends
/// in `_` plus) one of api_key, apikey, token, secret, password, webhook,
/// webhook_url, authorization; or it is `key` under `providers`; or it sits
/// directly in a `headers` table.
pub fn is_secret_key(path: &[String]) -> bool {
    let Some(last) = path.last() else {
        return false;
    };
    let last = last.to_ascii_lowercase();
    let in_headers = path
        .iter()
        .rev()
        .nth(1)
        .is_some_and(|parent| parent.eq_ignore_ascii_case("headers"));
    let provider_key = last == "key" && path.first().is_some_and(|f| f == "providers");
    in_headers
        || provider_key
        || SECRET_WORDS
            .iter()
            .any(|word| last == *word || last.ends_with(&format!("_{word}")))
}

/// A whole-string `${NAME}` reference: printed as-is, it names a variable
/// and holds no secret.
fn is_env_ref(s: &str) -> bool {
    s.strip_prefix("${")
        .and_then(|rest| rest.strip_suffix('}'))
        .is_some_and(|name| !name.is_empty() && !name.contains(['$', '{', '}']))
}

/// `value`, found at `path`, with every secret string under it replaced by
/// [`REDACTED`]. `${NAME}` references are kept.
pub fn redact(value: &Value, path: &[String]) -> Value {
    redact_under(value, &mut path.to_vec(), false)
}

fn redact_under(value: &Value, path: &mut Vec<String>, inherited: bool) -> Value {
    let secret = inherited || is_secret_key(path);
    match value {
        Value::String(s) if secret && !is_env_ref(s) => Value::String(REDACTED.to_string()),
        Value::Table(table) => Value::Table(
            table
                .iter()
                .map(|(k, v)| {
                    path.push(k.clone());
                    let out = redact_under(v, path, secret);
                    path.pop();
                    (k.clone(), out)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| redact_under(item, path, secret))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// A value for people: strings bare, tables as a TOML document, the rest
/// as a TOML literal.
pub fn render(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Table(table) => table.to_string(),
        other => other.to_string(),
    }
}

/// Set `path` in `doc` to `value`. Missing tables are created; a segment
/// after an array of tables picks the element by index or `name`. The
/// replaced value keeps its old spacing and trailing comment.
pub(crate) fn set_in_document(
    doc: &mut DocumentMut,
    path: &[String],
    mut value: toml_edit::Value,
) -> Result<()> {
    let (last, parents) = path
        .split_last()
        .ok_or_else(|| Error::Config("empty config key".to_string()))?;
    let mut table: &mut dyn TableLike = doc.as_table_mut();
    let mut inline = false;
    let mut segs = parents.iter();
    while let Some(seg) = segs.next() {
        let item = table.entry(seg).or_insert(new_table(inline));
        inline = item.is_inline_table();
        table = match item {
            Item::ArrayOfTables(array) => {
                let pick = segs.next().ok_or_else(|| not_a_table(seg))?;
                let found = match pick.parse::<usize>().ok() {
                    Some(index) => array.get_mut(index),
                    None => array
                        .iter_mut()
                        .find(|t| t.get("name").and_then(Item::as_str) == Some(pick.as_str())),
                };
                found.ok_or_else(|| {
                    Error::Config(format!("no '{seg}' entry named '{pick}' in this file"))
                })?
            }
            other => other.as_table_like_mut().ok_or_else(|| not_a_table(seg))?,
        };
    }
    let old_decor = table
        .get(last)
        .and_then(Item::as_value)
        .map(|v| v.decor().clone());
    *value.decor_mut() = old_decor.unwrap_or_default();
    // Replace in place so the key keeps its position and leading comments.
    match table.get_mut(last) {
        Some(item) => *item = Item::Value(value),
        None => {
            table.insert(last, Item::Value(value));
        }
    }
    Ok(())
}

fn new_table(inline: bool) -> Item {
    if inline {
        return Item::Value(toml_edit::Value::InlineTable(Default::default()));
    }
    let mut table = toml_edit::Table::new();
    table.set_implicit(true);
    Item::Table(table)
}

fn not_a_table(seg: &str) -> Error {
    Error::Config(format!("'{seg}' is not a table"))
}

#[cfg(test)]
#[path = "dotted_tests.rs"]
mod tests;
