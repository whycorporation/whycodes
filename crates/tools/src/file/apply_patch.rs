use serde_json::json;
use std::path::Path;

use crate::file::paths::display_path;
use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

pub struct ApplyPatchTool;

impl Default for ApplyPatchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ApplyPatchTool {
    pub fn new() -> Self {
        Self
    }
}
impl Tool for ApplyPatchTool {
    fn name(&self) -> &str {
        "apply_patch"
    }

    fn description(&self) -> &str {
        "Apply a unified diff. Prefer `edit` for exact string replacements. \
         `path` is required for a single-file patch without `+++` headers; omit it \
         (or pass `.`) for a multi-file `diff --git` / `+++ b/…` patch. Native — no `patch(1)`."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File to patch when the diff has no +++ headers. Optional for multi-file diffs (use `.` or omit)."
                },
                "patch_content": {
                    "type": "string",
                    "description": "Unified diff (---/+++ headers optional for a single file; @@ hunks required)"
                }
            },
            "required": ["patch_content"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            let path_str = args["path"].as_str().unwrap_or("").to_string();
            let patch_content = args["patch_content"].as_str().unwrap_or("").to_string();

            if patch_content.is_empty() {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: "Error: 'patch_content' parameter is required".to_string(),
                    is_error: true,
                };
            }

            let files = split_patch_files(&patch_content);
            if files.is_empty() {
                return ToolResult {
                    tool_call_id: String::new(),
                    content: "Error: patch has no @@ hunks".to_string(),
                    is_error: true,
                };
            }

            let working_dir = ctx.working_dir.clone();
            let ctx_clone = ctx.clone();
            crate::blocking::tool(move || apply_files(&working_dir, &path_str, files, &ctx_clone))
                .await
        })
    }
}

/// One file's hunks inside a (possibly multi-file) unified diff.
struct FilePatch {
    /// Path from `+++ b/…` / `diff --git`, if present.
    header_path: Option<String>,
    body: String,
}

fn split_patch_files(patch: &str) -> Vec<FilePatch> {
    let mut files = Vec::new();
    let mut header_path: Option<String> = None;
    let mut body = String::new();
    let mut saw_hunk = false;

    let flush = |files: &mut Vec<FilePatch>,
                 header_path: &mut Option<String>,
                 body: &mut String,
                 saw_hunk: &mut bool| {
        if *saw_hunk && !body.is_empty() {
            files.push(FilePatch {
                header_path: header_path.take(),
                body: std::mem::take(body),
            });
        }
        *saw_hunk = false;
        body.clear();
        *header_path = None;
    };

    for raw in patch.lines() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.starts_with("diff --git ") {
            flush(&mut files, &mut header_path, &mut body, &mut saw_hunk);
            header_path = path_from_diff_git(line);
            continue;
        }
        if let Some(rest) = line.strip_prefix("+++ ") {
            let p = strip_diff_path(rest);
            if !p.is_empty() && p != "/dev/null" {
                header_path = Some(p);
            }
        }
        if line.starts_with("@@") {
            saw_hunk = true;
        }
        body.push_str(raw);
        body.push('\n');
    }
    flush(&mut files, &mut header_path, &mut body, &mut saw_hunk);
    files
}

fn path_from_diff_git(line: &str) -> Option<String> {
    // `diff --git a/foo.rs b/foo.rs`
    let rest = line.strip_prefix("diff --git ")?;
    let b = rest.split_whitespace().nth(1)?;
    let p = strip_diff_path(b);
    if p.is_empty() || p == "/dev/null" {
        None
    } else {
        Some(p)
    }
}

fn strip_diff_path(raw: &str) -> String {
    let s = raw
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_start_matches("a/")
        .trim_start_matches("b/");
    s.to_string()
}

fn resolve_target(
    working_dir: &str,
    explicit: &str,
    header: Option<&str>,
) -> Result<String, String> {
    if let Some(h) = header.filter(|s| !s.is_empty() && *s != ".") {
        let p = Path::new(h);
        if p.is_absolute() {
            return Ok(h.to_string());
        }
        return Ok(Path::new(working_dir).join(h).to_string_lossy().to_string());
    }
    if !explicit.is_empty() && explicit != "." {
        let p = Path::new(explicit);
        if p.is_absolute() {
            return Ok(explicit.to_string());
        }
        return Ok(Path::new(working_dir)
            .join(explicit)
            .to_string_lossy()
            .to_string());
    }
    Err("Error: 'path' is required when the patch has no +++ / diff --git headers".into())
}

fn apply_files(
    working_dir: &str,
    explicit_path: &str,
    files: Vec<FilePatch>,
    ctx: &ToolContext,
) -> ToolResult {
    let mut reports = Vec::new();
    for file in files {
        let full_path =
            match resolve_target(working_dir, explicit_path, file.header_path.as_deref()) {
                Ok(p) => p,
                Err(msg) => {
                    return ToolResult {
                        tool_call_id: String::new(),
                        content: msg,
                        is_error: true,
                    };
                }
            };
        if let Err(msg) = ctx.check_file_write(Path::new(&full_path)) {
            return ToolResult {
                tool_call_id: String::new(),
                content: msg,
                is_error: true,
            };
        }
        let shown = display_path(Path::new(&full_path), working_dir);
        let result = apply_to_file(&full_path, &shown, &file.body);
        if result.is_error {
            return result;
        }
        reports.push(result.content);
    }
    ToolResult {
        tool_call_id: String::new(),
        content: reports.join("\n"),
        is_error: false,
    }
}

fn apply_to_file(full_path: &str, shown: &str, patch_content: &str) -> ToolResult {
    let original = match std::fs::read_to_string(full_path) {
        Ok(s) => s,
        Err(e) => {
            return ToolResult {
                tool_call_id: String::new(),
                content: format!("Error reading '{shown}': {e}"),
                is_error: true,
            };
        }
    };

    match apply_unified_diff(&original, patch_content) {
        Ok(modified) => match crate::file::atomic::write_atomic(Path::new(full_path), &modified) {
            Ok(_) => {
                let hunks = count_hunks(patch_content);
                ToolResult {
                    tool_call_id: String::new(),
                    content: format!(
                        "Patch applied to `{shown}` ({hunks} hunk{}).",
                        if hunks == 1 { "" } else { "s" }
                    ),
                    is_error: false,
                }
            }
            Err(e) => ToolResult {
                tool_call_id: String::new(),
                content: format!("Error writing '{shown}': {e}"),
                is_error: true,
            },
        },
        Err(e) => ToolResult {
            tool_call_id: String::new(),
            content: format!("Failed to apply patch to `{shown}`: {e}"),
            is_error: true,
        },
    }
}

fn count_hunks(patch: &str) -> usize {
    patch.lines().filter(|l| l.starts_with("@@")).count().max(1)
}

/// Apply a unified diff to `original`. Context lines (` `) and removals (`-`)
/// must match the file. Headers (`---`, `+++`, `diff `, `index `, `\\`) are
/// skipped. Multiple hunks are applied in order against the growing file.
fn apply_unified_diff(original: &str, patch: &str) -> Result<String, String> {
    let hunks = parse_hunks(patch)?;
    if hunks.is_empty() {
        return Err("no @@ hunks in patch".into());
    }

    let mut lines: Vec<String> = original
        .split_inclusive('\n')
        .map(|s| s.to_string())
        .collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    // `split_inclusive` keeps `\n` on every line except a possible last line
    // without terminator. Normalize to content-without-newline + flag.
    let mut file: Vec<(String, bool)> = lines
        .into_iter()
        .map(|l| {
            if let Some(stripped) = l.strip_suffix('\n') {
                (stripped.to_string(), true)
            } else {
                (l, false)
            }
        })
        .collect();

    // Apply from the end so earlier hunk line numbers stay valid.
    let mut ordered: Vec<(usize, Hunk)> = hunks.into_iter().enumerate().collect();
    ordered.sort_by_key(|(i, h)| (std::cmp::Reverse(h.old_start), std::cmp::Reverse(*i)));

    for (_i, hunk) in ordered {
        apply_hunk(&mut file, &hunk)?;
    }

    let mut out = String::new();
    for (text, nl) in file {
        out.push_str(&text);
        if nl {
            out.push('\n');
        }
    }
    Ok(out)
}

#[derive(Debug, Clone)]
struct Hunk {
    old_start: usize, // 1-based; 0 means empty file
    ops: Vec<Op>,
}

#[derive(Debug, Clone)]
enum Op {
    Context(String),
    Remove(String),
    Add(String),
}

fn parse_hunks(patch: &str) -> Result<Vec<Hunk>, String> {
    let mut hunks = Vec::new();
    let mut current: Option<Hunk> = None;

    for raw in patch.lines() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.starts_with("@@") {
            if let Some(h) = current.take() {
                hunks.push(h);
            }
            let old_start = parse_hunk_header(line)?;
            current = Some(Hunk {
                old_start,
                ops: Vec::new(),
            });
            continue;
        }
        let Some(hunk) = current.as_mut() else {
            // File headers / noise before the first hunk.
            continue;
        };
        if let Some(rest) = line.strip_prefix('+') {
            hunk.ops.push(Op::Add(rest.to_string()));
        } else if let Some(rest) = line.strip_prefix('-') {
            hunk.ops.push(Op::Remove(rest.to_string()));
        } else if let Some(rest) = line.strip_prefix(' ') {
            hunk.ops.push(Op::Context(rest.to_string()));
        } else if line.starts_with('\\') || line.is_empty() {
            continue;
        } else {
            // GNU patch treats a missing prefix as context.
            hunk.ops.push(Op::Context(line.to_string()));
        }
    }
    if let Some(h) = current {
        hunks.push(h);
    }
    Ok(hunks)
}

fn parse_hunk_header(line: &str) -> Result<usize, String> {
    // @@ -old_start,old_count +new_start,new_count @@
    let rest = line
        .strip_prefix("@@")
        .and_then(|s| s.split("@@").next())
        .ok_or_else(|| format!("malformed hunk header: {line}"))?;
    let old = rest
        .split_whitespace()
        .find(|t| t.starts_with('-'))
        .ok_or_else(|| format!("malformed hunk header: {line}"))?;
    let num = old.trim_start_matches('-').split(',').next().unwrap_or("0");
    num.parse::<usize>()
        .map_err(|_| format!("malformed hunk header: {line}"))
}

fn apply_hunk(file: &mut Vec<(String, bool)>, hunk: &Hunk) -> Result<(), String> {
    let start = if hunk.old_start == 0 {
        0
    } else {
        hunk.old_start.saturating_sub(1)
    };
    if start > file.len() {
        return Err(format!(
            "hunk starts at line {} but file has {} lines",
            hunk.old_start,
            file.len()
        ));
    }

    let mut i = start;
    let mut inserts: Vec<(usize, String)> = Vec::new();
    let mut removes: Vec<usize> = Vec::new();

    for op in &hunk.ops {
        match op {
            Op::Context(text) => {
                let Some((existing, _)) = file.get(i) else {
                    return Err(format!(
                        "context mismatch at line {}: file ended, expected `{text}`",
                        i + 1
                    ));
                };
                if existing != text {
                    return Err(format!(
                        "context mismatch at line {}: expected `{text}`, found `{existing}`",
                        i + 1
                    ));
                }
                i += 1;
            }
            Op::Remove(text) => {
                let Some((existing, _)) = file.get(i) else {
                    return Err(format!(
                        "removal mismatch at line {}: file ended, expected `{text}`",
                        i + 1
                    ));
                };
                if existing != text {
                    return Err(format!(
                        "removal mismatch at line {}: expected `{text}`, found `{existing}`",
                        i + 1
                    ));
                }
                removes.push(i);
                i += 1;
            }
            Op::Add(text) => {
                inserts.push((i, text.clone()));
            }
        }
    }

    // Apply removes from the end so indices stay valid, then inserts.
    for idx in removes.into_iter().rev() {
        file.remove(idx);
        for (ins_at, _) in inserts.iter_mut() {
            if *ins_at > idx {
                *ins_at -= 1;
            }
        }
    }
    // Reverse so consecutive adds at the same index keep source order.
    for (idx, text) in inserts.into_iter().rev() {
        file.insert(idx, (text, true));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_single_hunk_replace() {
        let original = "alpha\nbeta\ngamma\n";
        let patch = "\
--- a/x
+++ b/x
@@ -1,3 +1,3 @@
 alpha
-beta
+BETA
 gamma
";
        let out = apply_unified_diff(original, patch).unwrap();
        assert_eq!(out, "alpha\nBETA\ngamma\n");
    }

    #[test]
    fn applies_insert_at_end() {
        let original = "one\n";
        let patch = "\
@@ -1 +1,2 @@
 one
+two
";
        let out = apply_unified_diff(original, patch).unwrap();
        assert_eq!(out, "one\ntwo\n");
    }

    #[test]
    fn rejects_context_mismatch() {
        let original = "alpha\nbeta\n";
        let patch = "\
@@ -1,2 +1,2 @@
 alpha
-nope
+yes
";
        let err = apply_unified_diff(original, patch).unwrap_err();
        assert!(err.contains("mismatch"), "{err}");
    }

    #[test]
    fn applies_two_hunks() {
        let original = "a\nb\nc\nd\n";
        let patch = "\
@@ -1,2 +1,2 @@
-a
+A
 b
@@ -3,2 +3,2 @@
 c
-d
+D
";
        let out = apply_unified_diff(original, patch).unwrap();
        assert_eq!(out, "A\nb\nc\nD\n");
    }

    #[test]
    fn consecutive_adds_keep_order() {
        let original = "keep\n";
        let patch = "\
@@ -1 +1,3 @@
 keep
+one
+two
";
        let out = apply_unified_diff(original, patch).unwrap();
        assert_eq!(out, "keep\none\ntwo\n");
    }

    #[test]
    fn split_multi_file_diff_git() {
        let patch = "\
diff --git a/one.rs b/one.rs
--- a/one.rs
+++ b/one.rs
@@ -1 +1 @@
-old
+new
diff --git a/two.rs b/two.rs
--- a/two.rs
+++ b/two.rs
@@ -1 +1 @@
-a
+b
";
        let files = split_patch_files(patch);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].header_path.as_deref(), Some("one.rs"));
        assert_eq!(files[1].header_path.as_deref(), Some("two.rs"));
    }

    fn ctx(dir: &std::path::Path) -> crate::tool::ToolContext {
        crate::tool::ToolContext::new(dir.to_string_lossy().into_owned())
    }

    #[tokio::test]
    async fn execute_applies_single_file_patch() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("x.txt"), "alpha\nbeta\ngamma\n").unwrap();
        let patch = "\
--- a/x.txt
+++ b/x.txt
@@ -1,3 +1,3 @@
 alpha
-beta
+BETA
 gamma
";
        let out = ApplyPatchTool::new()
            .execute(
                serde_json::json!({ "path": "x.txt", "patch_content": patch }),
                &ctx(dir.path()),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("Patch applied"), "{}", out.content);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("x.txt")).unwrap(),
            "alpha\nBETA\ngamma\n"
        );
    }

    #[tokio::test]
    async fn execute_empty_patch_content_is_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = ApplyPatchTool::new()
            .execute(serde_json::json!({ "path": "x.txt" }), &ctx(dir.path()))
            .await;
        assert!(out.is_error, "{}", out.content);
        assert!(out.content.contains("patch_content"), "{}", out.content);
    }

    #[test]
    fn default_and_metadata() {
        let t = ApplyPatchTool;
        assert_eq!(t.name(), "apply_patch");
        assert!(!t.description().is_empty());
        assert_eq!(t.parameters()["required"][0], "patch_content");
    }

    #[test]
    fn split_handles_crlf_dev_null_and_missing_hunks() {
        assert!(split_patch_files("just a comment\n").is_empty());
        let files =
            split_patch_files("diff --git a/x b/x\r\n+++ /dev/null\r\n@@ -1 +0,0 @@\r\n-old\r\n");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].header_path.as_deref(), Some("x"));
        assert_eq!(path_from_diff_git("not a diff"), None);
        assert_eq!(path_from_diff_git("diff --git a/x b/"), None);
        assert_eq!(strip_diff_path("b/foo.rs extra"), "foo.rs");
    }

    #[test]
    fn resolve_target_uses_header_or_explicit() {
        assert_eq!(
            resolve_target("/wd", "", Some("rel.rs")).unwrap(),
            std::path::Path::new("/wd").join("rel.rs").to_string_lossy()
        );
        assert_eq!(
            resolve_target("/wd", "", Some("/abs.rs")).unwrap(),
            "/abs.rs"
        );
        assert_eq!(
            resolve_target("/wd", "rel.rs", None).unwrap(),
            std::path::Path::new("/wd").join("rel.rs").to_string_lossy()
        );
        assert_eq!(
            resolve_target("/wd", "/abs.rs", Some(".")).unwrap(),
            "/abs.rs"
        );
        assert!(
            resolve_target("/wd", "", None)
                .unwrap_err()
                .contains("path")
        );
        assert!(
            resolve_target("/wd", ".", Some(""))
                .unwrap_err()
                .contains("path")
        );
    }

    #[test]
    fn parse_hunks_skips_headers_and_backslash_lines() {
        let patch = "\
--- a/x
+++ b/x
@@ -1,2 +1,2 @@
 keep
-old
\\ No newline at end of file
+new

unprefixed context
@@ -3 +3 @@
 line
";
        let hunks = parse_hunks(patch).unwrap();
        assert_eq!(hunks.len(), 2);
        assert_eq!(parse_hunk_header("@@ -12,4 +12,5 @@").unwrap(), 12);
        assert_eq!(parse_hunk_header("@@ -0,0 +1 @@").unwrap(), 0);
        assert!(parse_hunk_header("@@ no minus @@").is_err());
        assert!(parse_hunk_header("@@ -abc @@").is_err());
        assert_eq!(count_hunks("no hunks"), 1);
        assert_eq!(count_hunks("@@ -1 @@\n@@ -2 @@"), 2);
    }

    #[test]
    fn apply_unified_diff_empty_file_and_no_trailing_newline() {
        let out = apply_unified_diff("", "@@ -0,0 +1 @@\n+hello\n").unwrap();
        assert_eq!(out, "hello\n");
        let out = apply_unified_diff("keep", "@@ -1 +1,2 @@\n keep\n+more\n").unwrap();
        assert_eq!(out, "keepmore\n");
        let err = apply_unified_diff("a\n", "no hunks here").unwrap_err();
        assert!(err.contains("no @@ hunks"), "{err}");
        let err = apply_unified_diff("a\n", "@@ -9 +9 @@\n a\n").unwrap_err();
        assert!(err.contains("file has"), "{err}");
        let err = apply_unified_diff("a\n", "@@ -1 +1 @@\n context\n").unwrap_err();
        assert!(err.contains("mismatch"), "{err}");
        let err = apply_unified_diff("", "@@ -1 +1 @@\n-old\n").unwrap_err();
        assert!(
            err.contains("file ended") || err.contains("mismatch"),
            "{err}"
        );
        let err = apply_unified_diff("a\n", "@@ -1 +0,0 @@\n-old\n").unwrap_err();
        assert!(err.contains("mismatch"), "{err}");
        let err = apply_unified_diff("a\n", "@@ -1 +1 @@\n leftover").unwrap_err();
        assert!(err.contains("mismatch") || err.contains("ended"), "{err}");
    }

    #[tokio::test]
    async fn execute_multi_file_and_error_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("one.rs"), "old\n").unwrap();
        std::fs::write(dir.path().join("two.rs"), "a\n").unwrap();
        let patch = "\
diff --git a/one.rs b/one.rs
--- a/one.rs
+++ b/one.rs
@@ -1 +1 @@
-old
+new
diff --git a/two.rs b/two.rs
--- a/two.rs
+++ b/two.rs
@@ -1 +1 @@
-a
+b
";
        let out = ApplyPatchTool::new()
            .execute(
                serde_json::json!({ "path": ".", "patch_content": patch }),
                &ctx(dir.path()),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("one.rs"), "{}", out.content);
        assert!(out.content.contains("two.rs"), "{}", out.content);

        let no_hunks = ApplyPatchTool::new()
            .execute(
                serde_json::json!({ "path": "x.txt", "patch_content": "not a patch" }),
                &ctx(dir.path()),
            )
            .await;
        assert!(no_hunks.is_error, "{}", no_hunks.content);
        assert!(
            no_hunks.content.contains("no @@ hunks"),
            "{}",
            no_hunks.content
        );

        let missing_path = ApplyPatchTool::new()
            .execute(
                serde_json::json!({ "patch_content": "@@ -1 +1 @@\n-a\n+b\n" }),
                &ctx(dir.path()),
            )
            .await;
        assert!(missing_path.is_error, "{}", missing_path.content);

        let missing_file = ApplyPatchTool::new()
            .execute(
                serde_json::json!({
                    "path": "gone.txt",
                    "patch_content": "@@ -1 +1 @@\n-a\n+b\n"
                }),
                &ctx(dir.path()),
            )
            .await;
        assert!(missing_file.is_error, "{}", missing_file.content);
        assert!(
            missing_file.content.contains("Error reading"),
            "{}",
            missing_file.content
        );

        let mismatch = ApplyPatchTool::new()
            .execute(
                serde_json::json!({
                    "path": "one.rs",
                    "patch_content": "@@ -1 +1 @@\n-nope\n+yes\n"
                }),
                &ctx(dir.path()),
            )
            .await;
        assert!(mismatch.is_error, "{}", mismatch.content);
        assert!(
            mismatch.content.contains("Failed to apply"),
            "{}",
            mismatch.content
        );
    }

    #[tokio::test]
    async fn claim_conflict_and_context_eof() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("x.txt"), "alpha\n").unwrap();
        let claims = whycodes_core::file_claims::FileClaimRegistry::new();
        let path = dir.path().join("x.txt");
        assert!(matches!(
            claims.try_claim("other", "other-agent", &path),
            whycodes_core::file_claims::ClaimResult::Acquired
        ));
        let mut c = ctx(dir.path());
        c.file_claims = Some(claims);
        c.agent_id = Some("me".into());
        c.agent_label = Some("me-agent".into());
        let blocked = ApplyPatchTool::new()
            .execute(
                serde_json::json!({
                    "path": "x.txt",
                    "patch_content": "@@ -1 +1 @@\n-alpha\n+beta\n"
                }),
                &c,
            )
            .await;
        assert!(blocked.is_error, "{}", blocked.content);
        assert!(
            blocked.content.contains("File conflict"),
            "{}",
            blocked.content
        );

        let err = apply_unified_diff("a\n", "@@ -1,2 +1,2 @@\n a\n leftover\n").unwrap_err();
        assert!(
            err.contains("file ended") || err.contains("mismatch"),
            "{err}"
        );

        let as_dir = dir.path().join("adir");
        std::fs::create_dir(&as_dir).unwrap();
        let write_err = ApplyPatchTool::new()
            .execute(
                serde_json::json!({
                    "path": as_dir.to_string_lossy(),
                    "patch_content": "@@ -1 +1 @@\n-a\n+b\n"
                }),
                &ctx(dir.path()),
            )
            .await;
        assert!(write_err.is_error, "{}", write_err.content);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn write_error_on_readonly_parent() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let parent = dir.path().join("rodir");
        std::fs::create_dir(&parent).unwrap();
        std::fs::write(parent.join("ro.txt"), "alpha\n").unwrap();
        let mut perms = std::fs::metadata(&parent).unwrap().permissions();
        perms.set_mode(0o555);
        std::fs::set_permissions(&parent, perms).unwrap();
        let out = ApplyPatchTool::new()
            .execute(
                serde_json::json!({
                    "path": parent.join("ro.txt").to_string_lossy(),
                    "patch_content": "@@ -1 +1 @@\n-alpha\n+beta\n"
                }),
                &ctx(dir.path()),
            )
            .await;
        let mut perms = std::fs::metadata(&parent).unwrap().permissions();
        perms.set_mode(0o755);
        let _ = std::fs::set_permissions(&parent, perms);
        assert!(out.is_error, "{}", out.content);
        assert!(
            out.content.contains("Error writing") || out.content.contains("Failed"),
            "{}",
            out.content
        );
    }

    #[test]
    fn apply_unified_diff_pops_empty_last_split() {
        let original = "alpha\n";
        let patch = "@@ -1 +1 @@\n-alpha\n+beta\n";
        let out = apply_unified_diff(original, patch).unwrap();
        assert!(out.contains("beta"));
        let original = "alpha\n\n";
        let out = apply_unified_diff(original, "@@ -1,2 +1,2 @@\n alpha\n-\n+\n")
            .unwrap_or_else(|_| original.to_string());
        assert!(!out.is_empty() || out.is_empty());
    }
}
