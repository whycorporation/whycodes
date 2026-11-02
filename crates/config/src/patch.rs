//! Save an edited [`crate::Config`] by patching the file it was read from.
//!
//! Serializing the whole struct (`Config::save`) wrote every default the
//! file never had, dropped comments and reordered tables: one `mcp add`
//! turned a hand-written `config.toml` into a generated one. Here only the
//! keys whose values differ between the config before and after the edit
//! are written; every other key, comment and blank line stays.

use toml::Value;
use toml_edit::{DocumentMut, Item, TableLike};
use whycodes_core::Result;

use crate::load::toml_err;

/// `text` with the changes from `before` to `after` applied.
pub(crate) fn patched_text(text: &str, before: &Value, after: &Value) -> Result<String> {
    let mut doc: DocumentMut = text.parse().map_err(|e| toml_err(format!("{e}")))?;
    if let (Value::Table(before), Value::Table(after)) = (before, after) {
        patch_table(doc.as_table_mut(), before, after)?;
    }
    Ok(doc.to_string())
}

fn patch_table(doc: &mut dyn TableLike, before: &toml::Table, after: &toml::Table) -> Result<()> {
    for (key, new) in after {
        let old = before.get(key);
        if old == Some(new) {
            continue;
        }
        // A table the file already has: recurse, so its other keys and
        // comments stay. Anything else is written whole.
        let nested = match (old, new, doc.get_mut(key)) {
            (Some(Value::Table(old)), Value::Table(new), Some(item)) => {
                item.as_table_like_mut().map(|table| (table, old, new))
            }
            _ => None,
        };
        match nested {
            Some((table, old, new)) => patch_table(table, old, new)?,
            None => {
                doc.insert(key, to_item(new)?);
            }
        }
    }
    for key in before.keys().filter(|key| !after.contains_key(*key)) {
        doc.remove(key);
    }
    Ok(())
}

/// `value` as a document item, written the way `toml` writes it.
fn to_item(value: &Value) -> Result<Item> {
    let mut wrap = toml::Table::new();
    wrap.insert("v".into(), value.clone());
    let text = toml::to_string(&wrap).map_err(|e| toml_err(e.to_string()))?;
    let mut doc: DocumentMut = text.parse().map_err(|e| toml_err(format!("{e}")))?;
    Ok(doc.remove("v").unwrap_or_default())
}

#[cfg(test)]
#[path = "patch_tests.rs"]
mod tests;
