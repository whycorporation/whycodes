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
        "Make targeted edits to a file. Prefer `from`/`to`/`insert_after` \
         content tags from `read`/`grep` (`N tag|text`). `old_string` is \
         the fallback: exact match, then unique whitespace-tolerant match \
         (indent / extra spaces only — not typos)."
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
                    "description": "Exact text to find. Required unless from/to/insert_after is set."
                },
                "new_string": {
                    "type": "string",
                    "description": "The replacement or inserted text"
                },
                "from": {
                    "type": "string",
                    "description": "Content tag of the first line to replace (inclusive)"
                },
                "to": {
                    "type": "string",
                    "description": "Content tag of the last line to replace (inclusive). Omit for a single line."
                },
                "insert_after": {
                    "type": "string",
                    "description": "Content tag after which to insert new_string (no deletion)"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences of old_string (default: false)"
                }
            },
            "required": ["path", "new_string"]
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
            let from = args["from"].as_str().map(str::to_string);
            let to = args["to"].as_str().map(str::to_string);
            let insert_after = args["insert_after"].as_str().map(str::to_string);

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
                Self::run(
                    full_path,
                    shown,
                    old_string,
                    new_string,
                    replace_all,
                    from,
                    to,
                    insert_after,
                )
            })
            .await
        })
    }
}

impl EditTool {
    #[allow(clippy::too_many_arguments)]
    fn run(
        full_path: String,
        shown: String,
        old_string: String,
        new_string: String,
        replace_all: bool,
        from: Option<String>,
        to: Option<String>,
        insert_after: Option<String>,
    ) -> ToolResult {
        match std::fs::read_to_string(&full_path) {
            Ok(original) => {
                let tagged = from.is_some() || insert_after.is_some();
                if tagged {
                    match apply_tagged(
                        &original,
                        from.as_deref(),
                        to.as_deref(),
                        insert_after.as_deref(),
                        &new_string,
                    ) {
                        Ok((matched, modified, start)) => write_edit(
                            &full_path,
                            &shown,
                            &matched,
                            &new_string,
                            1,
                            start,
                            &modified,
                        ),
                        Err(msg) => ToolResult {
                            tool_call_id: String::new(),
                            content: msg,
                            is_error: true,
                        },
                    }
                } else {
                    match locate_spans(&original, &old_string, replace_all) {
                        Locate::None => ToolResult {
                            tool_call_id: String::new(),
                            content: miss_with_snippet(&original, &old_string),
                            is_error: true,
                        },
                        Locate::Ambiguous(count) => ToolResult {
                            tool_call_id: String::new(),
                            content: ambiguous_with_tags(&original, &old_string, count),
                            is_error: true,
                        },
                        Locate::Hits(spans) => {
                            let matched = original[spans[0].0..spans[0].1].to_string();
                            let count = spans.len();
                            let modified = apply_spans(&original, &spans, &new_string);
                            let start = first_line_number(&original, &matched);
                            write_edit(
                                &full_path,
                                &shown,
                                &matched,
                                &new_string,
                                count,
                                start,
                                &modified,
                            )
                        }
                    }
                }
            }
            Err(e) => ToolResult {
                tool_call_id: String::new(),
                content: format!("Error reading file: {e}"),
                is_error: true,
            },
        }
    }
}

fn write_edit(
    full_path: &str,
    shown: &str,
    matched: &str,
    new_string: &str,
    count: usize,
    start: Option<usize>,
    modified: &str,
) -> ToolResult {
    write_edit_result(
        crate::file::atomic::write_atomic(std::path::Path::new(full_path), modified)
            .map_err(|e| e.to_string()),
        shown,
        matched,
        new_string,
        count,
        start,
    )
}

fn write_edit_result(
    result: Result<(), String>,
    shown: &str,
    matched: &str,
    new_string: &str,
    count: usize,
    start: Option<usize>,
) -> ToolResult {
    match result {
        Ok(()) => ToolResult {
            tool_call_id: String::new(),
            content: format_edit_preview_at(shown, matched, new_string, count, start),
            is_error: false,
        },
        Err(e) => write_edit_error(&e),
    }
}

fn write_edit_error(e: &str) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: format!("Error writing file: {e}"),
        is_error: true,
    }
}

fn apply_tagged(
    original: &str,
    from: Option<&str>,
    to: Option<&str>,
    insert_after: Option<&str>,
    new_string: &str,
) -> Result<(String, String, Option<usize>), String> {
    use crate::file::line_tag::{
        MIN_LEN, TagHit, find_tag, line_offsets, line_text, nearby_snippet, tag_of,
    };

    if from.is_some() && insert_after.is_some() {
        return Err("Use either from/to or insert_after, not both.".into());
    }
    let offs = line_offsets(original);
    if offs.is_empty() {
        return Err("File is empty — tags cannot be resolved.".into());
    }

    let resolve = |tag: &str, label: &str| -> Result<usize, String> {
        match find_tag(original, &offs, tag) {
            TagHit::Unique(i) => Ok(i),
            TagHit::Missing => Err(format!(
                "Content tag `{tag}` ({label}) was not found. File may have changed.\n{}",
                nearby_snippet(original, &offs, 0, 4)
            )),
            TagHit::Ambiguous(hits) => {
                let mut msg = format!(
                    "Content tag `{tag}` ({label}) matches {} lines. Use a longer tag from the last `read`.\n",
                    hits.len()
                );
                for i in hits.iter().take(8) {
                    let text = line_text(original, offs[*i]);
                    let t = tag_of(text, MIN_LEN.max(tag.len()));
                    msg.push_str(&crate::file::line_tag::format_read_line(i + 1, &t, text));
                    msg.push('\n');
                }
                Err(msg)
            }
        }
    };

    if let Some(after) = insert_after {
        let i = resolve(after, "insert_after")?;
        let insert_at = offs[i].line_end;
        let mut insert = new_string.to_string();
        if !insert.is_empty() && !insert.ends_with('\n') {
            insert.push('\n');
        }
        if !insert.is_empty() && insert_at > 0 && !original[..insert_at].ends_with('\n') {
            insert.insert(0, '\n');
        }
        let mut modified = String::with_capacity(original.len() + insert.len());
        modified.push_str(&original[..insert_at]);
        modified.push_str(&insert);
        modified.push_str(&original[insert_at..]);
        return Ok((String::new(), modified, Some(i + 2)));
    }

    let from_tag =
        from.ok_or_else(|| "from is required when insert_after is not set.".to_string())?;
    let start_i = resolve(from_tag, "from")?;
    let end_i = if let Some(to_tag) = to {
        let j = resolve(to_tag, "to")?;
        if j < start_i {
            return Err("`to` must not precede `from`.".into());
        }
        j
    } else {
        start_i
    };
    let byte_start = offs[start_i].start;
    let byte_end = offs[end_i].line_end;
    let matched = original[byte_start..byte_end].to_string();
    let modified = apply_spans(original, &[(byte_start, byte_end)], new_string);
    Ok((matched, modified, Some(start_i + 1)))
}

fn miss_with_snippet(original: &str, old: &str) -> String {
    use crate::file::line_tag::{line_offsets, nearby_snippet};
    let offs = line_offsets(original);
    let mut center = 0usize;
    if let Some(tok) = old.split_whitespace().next()
        && let Some(pos) = original.find(tok)
    {
        center = original[..pos].bytes().filter(|&b| b == b'\n').count();
    }
    format!(
        "Could not find the specified text in the file.\nNearby lines:\n{}",
        nearby_snippet(original, &offs, center, 3)
    )
}

fn ambiguous_with_tags(original: &str, old: &str, count: usize) -> String {
    use crate::file::line_tag::{MIN_LEN, format_read_line, line_offsets, line_text, tag_of};
    let offs = line_offsets(original);
    let mut msg = format!(
        "Found {count} occurrences of the search text. Use replace_all=true, a more specific match, or a content tag.\n"
    );
    let mut shown = 0usize;
    let mut from = 0usize;
    while shown < 6 && from < original.len() {
        if let Some(rel) = original[from..].find(old) {
            let abs = from + rel;
            let line_i = original[..abs].bytes().filter(|&b| b == b'\n').count();
            if line_i < offs.len() {
                let text = line_text(original, offs[line_i]);
                let tag = tag_of(text, MIN_LEN);
                msg.push_str(&format_read_line(line_i + 1, &tag, text));
                msg.push('\n');
                shown += 1;
            }
            from = abs + old.len().max(1);
        } else {
            break;
        }
    }
    msg
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
            search = skip_token_at(start, first.len());
            continue;
        }
        if let Some(end) = tokens_match_after(haystack, &tokens[1..], start + first.len()) {
            return Some((start, end));
        }
        search = skip_token_at(start, first.len());
    }
    None
}

fn skip_token_at(start: usize, len: usize) -> usize {
    start + len
}

fn tokens_match_after(haystack: &str, tokens: &[&str], mut i: usize) -> Option<usize> {
    for tok in tokens {
        let after_ws = skip_ws(haystack, i);
        if after_ws == i {
            return tokens_need_ws();
        }
        if !haystack[after_ws..].starts_with(tok)
            || !right_boundary_ok(haystack, after_ws + tok.len(), tok)
        {
            return tokens_mismatch();
        }
        i = after_ws + tok.len();
    }
    Some(i)
}

fn skip_ws(s: &str, mut i: usize) -> usize {
    for ch in s[i..].chars() {
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

fn ident_boundary_missing() -> bool {
    true
}

fn left_boundary_ok(haystack: &str, start: usize, token: &str) -> bool {
    let Some(first) = token.chars().next() else {
        return false;
    };
    if start == 0 || !is_ident_char(first) {
        return true;
    }
    ident_left_ok(haystack, start)
}

fn ident_left_ok(haystack: &str, start: usize) -> bool {
    match haystack[..start].chars().next_back() {
        Some(prev) => !is_ident_char(prev),
        None => ident_boundary_missing(),
    }
}

fn tokens_need_ws() -> Option<usize> {
    None
}

fn tokens_mismatch() -> Option<usize> {
    None
}

fn right_boundary_ok(haystack: &str, end: usize, token: &str) -> bool {
    let Some(last) = token.chars().next_back() else {
        return false;
    };
    if end >= haystack.len() || !is_ident_char(last) {
        return true;
    }
    ident_right_ok(haystack, end)
}

fn ident_right_ok(haystack: &str, end: usize) -> bool {
    match haystack[end..].chars().next() {
        Some(next) => !is_ident_char(next),
        None => ident_boundary_missing(),
    }
}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;
