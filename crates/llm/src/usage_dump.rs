//! Optional dump of raw provider `usage` objects for live reconcile.
//!
//! Set `WHYCODE_USAGE_DUMP` to a file path (append JSONL) or `1`/`-` for
//! stderr. Unset or `0` disables the dump. One line per parsed usage object:
//!
//! ```json
//! {"source":"openai_compat","usage":{"prompt_tokens":12,"completion_tokens":4}}
//! ```
//!
//! The script `scripts/reconcile_token_usage.py` compares the last snapshot
//! against `whycode generate --format json` session usage.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use serde_json::{Value, json};

/// Write one raw usage object if `WHYCODE_USAGE_DUMP` is set.
pub fn dump_raw_usage(source: &str, usage: &Value) {
    if !usage.is_object() {
        return;
    }
    let Ok(dest) = std::env::var("WHYCODE_USAGE_DUMP") else {
        return;
    };
    if dest.is_empty() || dest == "0" {
        return;
    }
    let line = json!({ "source": source, "usage": usage });
    if dest == "1" || dest == "-" {
        if let Err(e) = writeln!(std::io::stderr(), "{line}") {
            tracing::debug!(error = %e, "usage dump stderr write failed");
        }
        return;
    }
    write_line(Path::new(&dest), &line);
}

pub(crate) fn write_line(path: &Path, line: &Value) {
    let mut file = match OpenOptions::new().create(true).append(true).open(path) {
        Ok(f) => f,
        Err(e) => {
            tracing::debug!(error = %e, path = %path.display(), "usage dump open failed");
            return;
        }
    };
    if let Err(e) = writeln!(file, "{line}") {
        tracing::debug!(error = %e, path = %path.display(), "usage dump write failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn write_line_appends_jsonl() {
        let dir = std::env::temp_dir().join(format!(
            "whycode-usage-dump-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("usage.jsonl");
        write_line(
            &path,
            &json!({"source":"openai_compat","usage":{"prompt_tokens":12,"completion_tokens":4}}),
        );
        let text = std::fs::read_to_string(&path).expect("dump file");
        let _ = std::fs::remove_dir_all(&dir);
        let parsed: Value = serde_json::from_str(text.lines().next().expect("one line")).unwrap();
        assert_eq!(parsed["source"], "openai_compat");
        assert_eq!(parsed["usage"]["prompt_tokens"], 12);
        assert_eq!(parsed["usage"]["completion_tokens"], 4);
    }

    #[test]
    fn dump_skips_non_objects() {
        dump_raw_usage("openai_compat", &Value::Null);
    }
}
