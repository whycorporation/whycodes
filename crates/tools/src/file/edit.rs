use serde_json::json;

use crate::file::paths::display_path;
use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;
use whycodes_format::diff::{first_line_number, format_edit_preview_at};

pub struct EditTool;

impl Default for EditTool {
    fn default() -> Self {
        Self::new()
    }
}

impl EditTool {
    pub fn new() -> Self {
        Self
    }
}
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Make targeted edits to a file by finding and replacing exact text. \
         If the exact `old_string` is missing, a unique whitespace-tolerant \
         match (indent / extra spaces only — not typos) is applied."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit"
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact text to find"
                },
                "new_string": {
                    "type": "string",
                    "description": "The replacement text"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences (default: false)"
                }
            },
            "required": ["path", "old_string", "new_string"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            let path_str = args["path"].as_str().unwrap_or("").to_string();
            let old_string = args["old_string"].as_str().unwrap_or("").to_string();
            let new_string = args["new_string"].as_str().unwrap_or("").to_string();
            let replace_all = args["replace_all"].as_bool().unwrap_or(false);

            let full_path = if std::path::Path::new(&path_str).is_absolute() {
                path_str
            } else {
                std::path::Path::new(&ctx.working_dir)
                    .join(&path_str)
                    .to_string_lossy()
                    .to_string()
            };

            if let Err(msg) = ctx.check_file_write(std::path::Path::new(&full_path)) {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: msg,
                    is_error: true,
                };
            }

            let shown = display_path(std::path::Path::new(&full_path), &ctx.working_dir);
            crate::blocking::tool(move || {
                Self::run(full_path, shown, old_string, new_string, replace_all)
            })
            .await
        })
    }
}

impl EditTool {
    fn run(
        full_path: String,
        shown: String,
        old_string: String,
        new_string: String,
        replace_all: bool,
    ) -> ToolResult {
        match std::fs::read_to_string(&full_path) {
            Ok(original) => match locate_spans(&original, &old_string, replace_all) {
                Locate::None => ToolResult {
                    tool_call_id: String::new(),
                    content: "Could not find the specified text in the file.".to_string(),
                    is_error: true,
                },
                Locate::Ambiguous(count) => ToolResult {
                    tool_call_id: String::new(),
                    content: format!(
                        "Found {count} occurrences of the search text. Use replace_all=true or provide a more specific match."
                    ),
                    is_error: true,
                },
                Locate::Hits(spans) => {
                    let matched = original[spans[0].0..spans[0].1].to_string();
                    let count = spans.len();
                    let modified = apply_spans(&original, &spans, &new_string);
                    let start = first_line_number(&original, &matched);
                    match crate::file::atomic::write_atomic(
                        std::path::Path::new(&full_path),
                        &modified,
                    ) {
                        Ok(()) => ToolResult {
                            tool_call_id: String::new(),
                            content: format_edit_preview_at(
                                &shown,
                                &matched,
                                &new_string,
                                count,
                                start,
                            ),
                            is_error: false,
                        },
                        Err(e) => ToolResult {
                            tool_call_id: String::new(),
                            content: format!("Error writing file: {e}"),
                            is_error: true,
                        },
                    }
                }
            },
            Err(e) => ToolResult {
                tool_call_id: String::new(),
                content: format!("Error reading file: {e}"),
                is_error: true,
            },
        }
    }
}

enum Locate {
    None,
    Ambiguous(usize),
    Hits(Vec<(usize, usize)>),
}

fn locate_spans(original: &str, old: &str, replace_all: bool) -> Locate {
    let exact = exact_spans(original, old);
    decide_spans(exact, original, old, replace_all)
}

fn decide_spans(
    exact: Vec<(usize, usize)>,
    original: &str,
    old: &str,
    replace_all: bool,
) -> Locate {
    if !exact.is_empty() {
        return finish_spans(exact, replace_all);
    }
    finish_spans(ws_flexible_spans(original, old), replace_all)
}

fn finish_spans(spans: Vec<(usize, usize)>, replace_all: bool) -> Locate {
    match spans.len() {
        0 => Locate::None,
        1 => Locate::Hits(spans),
        _ if replace_all => Locate::Hits(spans),
        n => Locate::Ambiguous(n),
    }
}

fn exact_spans(haystack: &str, needle: &str) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return Vec::new();
    }
    haystack
        .match_indices(needle)
        .map(|(i, s)| (i, i + s.len()))
        .collect()
}

fn apply_spans(original: &str, spans: &[(usize, usize)], new: &str) -> String {
    let mut out = String::with_capacity(original.len().saturating_add(new.len()));
    let mut last = 0;
    for &(start, end) in spans {
        out.push_str(&original[last..start]);
        out.push_str(new);
        last = end;
    }
    out.push_str(&original[last..]);
    out
}

/// Whitespace-tolerant matches: same non-whitespace tokens in order, with at
/// least one whitespace character between tokens. Indent / extra spaces / extra
/// blank lines may differ. Typos and glued tokens (`foo bar` vs `foobar`) do not
/// match. Requires ≥2 tokens so a lone identifier cannot fuzzy-replace.
fn ws_flexible_spans(haystack: &str, needle: &str) -> Vec<(usize, usize)> {
    let tokens: Vec<&str> = needle.split_whitespace().collect();
    if tokens.len() < 2 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut from = 0;
    while from < haystack.len() {
        match find_ws_flexible_from(haystack, &tokens, from) {
            Some((start, end)) => {
                out.push((start, end));
                from = end;
            }
            None => break,
        }
    }
    out
}

fn find_ws_flexible_from(haystack: &str, tokens: &[&str], from: usize) -> Option<(usize, usize)> {
    let first = tokens[0];
    let mut search = from;
    while search < haystack.len() {
        let rel = haystack[search..].find(first)?;
        let start = search + rel;
        if !left_boundary_ok(haystack, start, first)
            || !right_boundary_ok(haystack, start + first.len(), first)
        {
            search = start + first.len();
            continue;
        }
        let mut i = start + first.len();
        let mut ok = true;
        for tok in &tokens[1..] {
            let after_ws = skip_ws(haystack, i);
            if after_ws == i {
                ok = false;
                break;
            }
            if !haystack[after_ws..].starts_with(tok)
                || !right_boundary_ok(haystack, after_ws + tok.len(), tok)
            {
                ok = false;
                break;
            }
            i = after_ws + tok.len();
        }
        if ok {
            return Some((start, i));
        }
        search = start + first.len();
    }
    None
}

fn skip_ws(s: &str, mut i: usize) -> usize {
    while i < s.len() {
        let Some(ch) = s[i..].chars().next() else {
            break;
        };
        if !ch.is_whitespace() {
            break;
        }
        i += ch.len_utf8();
    }
    i
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn left_boundary_ok(haystack: &str, start: usize, token: &str) -> bool {
    let Some(first) = token.chars().next() else {
        return false;
    };
    if start == 0 || !is_ident_char(first) {
        return true;
    }
    match haystack[..start].chars().next_back() {
        Some(prev) => !is_ident_char(prev),
        None => true,
    }
}

fn right_boundary_ok(haystack: &str, end: usize, token: &str) -> bool {
    let Some(last) = token.chars().next_back() else {
        return false;
    };
    if end >= haystack.len() || !is_ident_char(last) {
        return true;
    }
    match haystack[end..].chars().next() {
        Some(next) => !is_ident_char(next),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_span_unique() {
        let spans = exact_spans("ab X cd X", "X");
        assert_eq!(spans, vec![(3, 4), (8, 9)]);
    }

    #[test]
    fn ws_flexible_matches_indent_and_spacing() {
        let file = "fn run() {\n    let x = 1;\n}\n";
        let needle = "fn run() {\n  let x = 1;\n}";
        let spans = ws_flexible_spans(file, needle);
        assert_eq!(spans.len(), 1);
        assert_eq!(
            &file[spans[0].0..spans[0].1],
            "fn run() {\n    let x = 1;\n}"
        );
    }

    #[test]
    fn ws_flexible_does_not_glue_tokens() {
        assert!(ws_flexible_spans("foobar", "foo bar").is_empty());
        assert!(ws_flexible_spans("fnord x", "fn x").is_empty());
    }

    #[test]
    fn ws_flexible_skips_single_token() {
        assert!(ws_flexible_spans("hello", "hello").is_empty());
    }

    #[test]
    fn ws_flexible_ambiguous_two_blocks() {
        let file = "fn a() {\n  x();\n}\nfn b() {\n  x();\n}\n";
        let needle = "x();";
        // single token → no fuzzy
        assert!(ws_flexible_spans(file, needle).is_empty());
        let needle = "{\n  x();\n}";
        assert_eq!(ws_flexible_spans(file, needle).len(), 2);
    }

    #[test]
    fn apply_spans_replaces_in_order() {
        let s = "aa X bb X cc";
        let out = apply_spans(s, &[(3, 4), (8, 9)], "Y");
        assert_eq!(out, "aa Y bb Y cc");
    }

    #[test]
    fn locate_prefers_exact_over_fuzzy() {
        let file = "foo  bar\nfoo bar\n";
        match locate_spans(file, "foo bar", false) {
            Locate::Hits(spans) => {
                assert_eq!(spans, vec![(9, 16)]);
            }
            _ => panic!("expected unique exact, got mismatch"),
        }
        match locate_spans(file, "foo bar", true) {
            Locate::Hits(spans) => assert_eq!(spans.len(), 1),
            _ => panic!("replace_all still exact-only when exact exists"),
        }
    }

    #[test]
    fn locate_fuzzy_when_exact_missing() {
        let file = "    foo   bar\n";
        match locate_spans(file, "foo bar", false) {
            Locate::Hits(spans) => assert_eq!(&file[spans[0].0..spans[0].1], "foo   bar"),
            _ => panic!("expected fuzzy hit"),
        }
    }

    fn ctx(dir: &std::path::Path) -> crate::tool::ToolContext {
        crate::tool::ToolContext::new(dir.to_string_lossy().into_owned())
    }

    #[tokio::test]
    async fn execute_replaces_text_and_reports_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("a.rs");
        std::fs::write(&path, "fn run() { let x = 1; }\n").unwrap();
        let tool = EditTool::new();
        let ok = tool
            .execute(
                serde_json::json!({
                    "path": "a.rs",
                    "old_string": "let x = 1;",
                    "new_string": "let x = 2;"
                }),
                &ctx(dir.path()),
            )
            .await;
        assert!(!ok.is_error, "{}", ok.content);
        assert!(
            ok.content.contains("a.rs") || !ok.content.is_empty(),
            "{}",
            ok.content
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "fn run() { let x = 2; }\n"
        );

        let miss = tool
            .execute(
                serde_json::json!({
                    "path": "a.rs",
                    "old_string": "definitely-not-here",
                    "new_string": "x"
                }),
                &ctx(dir.path()),
            )
            .await;
        assert!(miss.is_error, "{}", miss.content);
        assert!(miss.content.contains("Could not find"), "{}", miss.content);
    }

    #[tokio::test]
    async fn execute_missing_required_params_is_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = EditTool::new()
            .execute(serde_json::json!({"path": "nope.rs"}), &ctx(dir.path()))
            .await;
        assert!(out.is_error, "{}", out.content);
    }
}
