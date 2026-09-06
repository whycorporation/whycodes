//! Optional dump of raw provider `usage` objects for live reconcile.
//!
//! Set `WHYCODES_USAGE_DUMP` to a file path (append JSONL) or `1`/`-` for
//! stderr. Unset or `0` disables the dump. One line per parsed usage object:
//!
//! ```json
//! {"source":"openai_compat","usage":{"prompt_tokens":12,"completion_tokens":4}}
//! ```
//!
//! The script `scripts/reconcile_token_usage.py` compares the last snapshot
//! against `whycodes generate --format json` session usage.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use serde_json::Value;

/// Write one raw usage object if `WHYCODES_USAGE_DUMP` is set.
pub fn dump_raw_usage(source: &str, usage: &Value) {
    if !usage.is_object() {
        return;
    }
    let Ok(dest) = std::env::var("WHYCODES_USAGE_DUMP") else {
        return;
    };
    if dest.is_empty() || dest == "0" {
        return;
    }
    let line = crate::json_value::obj([
        ("source", crate::json_value::str(source)),
        ("usage", usage.clone()),
    ]);
    if dest == "1" || dest == "-" {
        write_value(std::io::stderr(), &line, "stderr");
        return;
    }
    write_line(Path::new(&dest), &line);
}

fn write_value(mut w: impl Write, line: &Value, dest: &str) {
    if let Err(e) = writeln!(w, "{line}") {
        tracing::debug!("usage dump write failed dest={dest} error={e}");
    }
}

pub(crate) fn write_line(path: &Path, line: &Value) {
    let file = match OpenOptions::new().create(true).append(true).open(path) {
        Ok(f) => f,
        Err(e) => {
            let path = path.display();
            tracing::debug!("usage dump open failed path={path} error={e}");
            return;
        }
    };
    write_value(file, line, &path.display().to_string());
}

#[cfg(test)]
#[path = "usage_dump_tests.rs"]
mod tests;
