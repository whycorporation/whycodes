use async_trait::async_trait;
use serde_json::json;
use std::path::Path;

use super::paths::{
    display_path, glob_match, human_size, is_skip_dir, list_dir_entries, resolve_path,
};
use crate::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

/// Default max entries returned for a single list call.
const DEFAULT_MAX_ENTRIES: usize = 200;
const HARD_MAX_ENTRIES: usize = 2000;
const DEFAULT_MAX_DEPTH: usize = 3;

/// List files and directories — OpenCode `list` tool equivalent.
pub struct ListTool;

impl Default for ListTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ListTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ListTool {
    fn name(&self) -> &str {
        "list"
    }

    fn description(&self) -> &str {
        "List files and directories in a path (dirs first, with sizes). \
         Prefer this over shell `ls`. Optional recursive listing respects .gitignore \
         and skips heavy dirs (target, node_modules, .git, …). Use `glob` for pattern search."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory path (relative to project root or absolute). Defaults to project root."
                },
                "ignore": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Glob patterns to ignore (e.g. '*.o', 'tmp*')"
                },
                "recursive": {
                    "type": "boolean",
                    "description": "Recurse into subdirectories (default: false). Heavy dirs are pruned."
                },
                "max_depth": {
                    "type": "integer",
                    "description": "Max recursion depth when recursive=true (default: 3)"
                },
                "max_entries": {
                    "type": "integer",
                    "description": "Max entries to return (default: 200)"
                }
            },
            "required": []
        })
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let rel = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let target = resolve_path(&ctx.working_dir, rel);
        let shown = display_path(&target, &ctx.working_dir);

        if !target.exists() {
            return ToolResult {
                tool_call_id: String::new(),
                content: format!("Path does not exist: {}", shown),
                is_error: true,
            };
        }
        if !target.is_dir() {
            return ToolResult {
                tool_call_id: String::new(),
                content: format!("Not a directory: {} (use `read` for files)", shown),
                is_error: true,
            };
        }

        let ignore: Vec<String> = args
            .get("ignore")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let recursive = args
            .get("recursive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let max_depth = args
            .get("max_depth")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_MAX_DEPTH)
            .clamp(1, 20);
        let max_entries = args
            .get("max_entries")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_MAX_ENTRIES)
            .clamp(1, HARD_MAX_ENTRIES);

        let (entries, truncated, dir_count, file_count) =
            if recursive {
                // Fast path: the warm workspace index already knows the tree.
                match ctx.file_index.as_deref().and_then(|idx| {
                    list_recursive_index(idx, &target, &ignore, max_depth, max_entries)
                }) {
                    Some(out) => out,
                    None => list_recursive(&target, &ignore, max_depth, max_entries),
                }
            } else {
                match list_dir_entries(&target, &ignore) {
                    Ok(all) => {
                        let dir_count = all.iter().filter(|e| e.is_dir).count();
                        let file_count = all.len() - dir_count;
                        let truncated = all.len() > max_entries;
                        let entries: Vec<(String, bool, Option<u64>)> = all
                            .into_iter()
                            .take(max_entries)
                            .map(|e| (e.name, e.is_dir, e.size))
                            .collect();
                        (entries, truncated, dir_count, file_count)
                    }
                    Err(e) => {
                        return ToolResult {
                            tool_call_id: String::new(),
                            content: e,
                            is_error: true,
                        };
                    }
                }
            };

        let mut out = format!("Contents of {}:\n", shown);
        if entries.is_empty() {
            out.push_str("(empty)\n");
        } else {
            // Column width for names
            let name_w = entries
                .iter()
                .map(|(n, is_dir, _)| n.len() + if *is_dir { 1 } else { 0 })
                .max()
                .unwrap_or(8)
                .min(60);

            for (name, is_dir, size) in &entries {
                if *is_dir {
                    out.push_str(&format!("  {:<width$}/\n", name, width = name_w));
                } else {
                    let sz = size.map(human_size).unwrap_or_else(|| "?".into());
                    out.push_str(&format!("  {:<width$}  {:>10}\n", name, sz, width = name_w));
                }
            }
        }

        out.push_str(&format!(
            "\n{} directories, {} files",
            dir_count, file_count
        ));
        if truncated {
            out.push_str(&format!(
                " (showing first {} — raise max_entries or narrow path)",
                entries.len()
            ));
        }
        if recursive {
            out.push_str(&format!(" [recursive depth≤{}]", max_depth));
        }

        ToolResult {
            tool_call_id: String::new(),
            content: out,
            is_error: false,
        }
    }
}

/// One listed entry: (display_name, is_dir, size).
type ListEntry = (String, bool, Option<u64>);

/// Recursive listing result: entries, truncated flag, dir count, file count.
type ListRecursiveOut = (Vec<ListEntry>, bool, usize, usize);

struct WalkState<'a> {
    entries: &'a mut Vec<ListEntry>,
    dir_count: &'a mut usize,
    file_count: &'a mut usize,
    truncated: &'a mut bool,
}

/// Index-backed recursive listing: same shape as [`list_recursive`] without
/// touching the filesystem. Returns None when the index is cold / out of
/// scope (caller falls back to the walk).
fn list_recursive_index(
    index: &whycode_index::WorkspaceIndex,
    root: &Path,
    ignore: &[String],
    max_depth: usize,
    max_entries: usize,
) -> Option<ListRecursiveOut> {
    let entries = super::paths::index_entries(index, root)?;
    let mut out: Vec<ListEntry> = Vec::new();
    let mut dir_count = 0usize;
    let mut file_count = 0usize;
    let mut truncated = false;
    for (_abs, rel, is_dir, size) in entries {
        // Depth in components: `a/b/c` is 3 (walk starts counting at 1).
        let depth = rel.bytes().filter(|b| *b == b'/').count() + 1;
        if depth > max_depth {
            continue;
        }
        let name = rel.rsplit('/').next().unwrap_or(&rel);
        if ignore.iter().any(|pat| glob_match(pat, name)) {
            continue;
        }
        if is_dir {
            dir_count += 1;
        } else {
            file_count += 1;
        }
        if out.len() >= max_entries {
            truncated = true;
            continue; // keep counting for accurate totals
        }
        out.push((rel, is_dir, if is_dir { None } else { Some(size) }));
    }
    out.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.to_ascii_lowercase().cmp(&b.0.to_ascii_lowercase()))
    });
    Some((out, truncated, dir_count, file_count))
}

/// Recursive listing with depth limit and skip-dir pruning.
/// Returns (display_name, is_dir, size), truncated flag, total dir/file counts.
fn list_recursive(
    root: &Path,
    ignore: &[String],
    max_depth: usize,
    max_entries: usize,
) -> ListRecursiveOut {
    let mut entries = Vec::new();
    let mut dir_count = 0usize;
    let mut file_count = 0usize;
    let mut truncated = false;

    fn walk(
        root: &Path,
        dir: &Path,
        depth: usize,
        max_depth: usize,
        ignore: &[String],
        max_entries: usize,
        state: &mut WalkState,
    ) {
        if *state.truncated {
            return;
        }
        let Ok(level) = list_dir_entries(dir, ignore) else {
            return;
        };
        for e in level {
            if e.is_dir {
                *state.dir_count += 1;
            } else {
                *state.file_count += 1;
            }

            if state.entries.len() >= max_entries {
                *state.truncated = true;
                return;
            }

            let rel = e
                .path
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| e.name.clone());

            state.entries.push((rel, e.is_dir, e.size));

            if e.is_dir && depth < max_depth && !is_skip_dir(&e.name) {
                walk(
                    root,
                    &e.path,
                    depth + 1,
                    max_depth,
                    ignore,
                    max_entries,
                    state,
                );
            }
        }
    }

    walk(
        root,
        root,
        1,
        max_depth,
        ignore,
        max_entries,
        &mut WalkState {
            entries: &mut entries,
            dir_count: &mut dir_count,
            file_count: &mut file_count,
            truncated: &mut truncated,
        },
    );

    // Keep dirs-first-ish: directories first, then name
    entries.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.to_ascii_lowercase().cmp(&b.0.to_ascii_lowercase()))
    });

    (entries, truncated, dir_count, file_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolContext;
    use std::time::Duration;
    use whycode_index::IndexOptions;

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir.to_string_lossy().into_owned())
    }

    fn ctx_with_index(dir: &std::path::Path) -> ToolContext {
        let idx = whycode_index::WorkspaceIndex::start_with(
            vec![dir.to_path_buf()],
            IndexOptions {
                watch: false,
                threads: 1,
                ..Default::default()
            },
        );
        assert!(
            idx.wait_ready(Duration::from_secs(10)),
            "index never became ready"
        );
        let mut c = ToolContext::new(dir.to_string_lossy().into_owned());
        c.file_index = Some(idx);
        c
    }

    #[tokio::test]
    async fn metadata_describes_list_tool() {
        let t = ListTool::new();
        assert_eq!(t.name(), "list");
        assert!(t.description().contains("List files"));
        let params = t.parameters();
        assert_eq!(params["required"], json!([]));
        assert!(params["properties"].get("path").is_some());
        assert!(params["properties"].get("recursive").is_some());
    }

    #[tokio::test]
    async fn lists_files_and_dirs_with_sizes() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), "hello").expect("write");
        std::fs::create_dir(dir.path().join("sub")).expect("mkdir");

        let out = ListTool::new().execute(json!({}), &ctx(dir.path())).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("Contents of"), "{}", out.content);
        assert!(out.content.contains("a.txt"), "{}", out.content);
        assert!(
            out.content.contains("sub") && out.content.contains('/'),
            "dir marked with slash: {}",
            out.content
        );
        assert!(
            out.content.contains("5 B"),
            "5-byte file shown: {}",
            out.content
        );
        assert!(
            out.content.contains("1 directories, 1 files"),
            "{}",
            out.content
        );
        assert!(
            !out.content.contains("[recursive"),
            "non-recursive has no depth tag: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn empty_directory_is_labelled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = ListTool::new().execute(json!({}), &ctx(dir.path())).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("(empty)"), "{}", out.content);
        assert!(
            out.content.contains("0 directories, 0 files"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn missing_path_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = ListTool::new()
            .execute(json!({ "path": "nope" }), &ctx(dir.path()))
            .await;
        assert!(out.is_error);
        assert!(
            out.content.contains("Path does not exist"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn file_path_is_not_a_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), "x").expect("write");
        let out = ListTool::new()
            .execute(json!({ "path": "a.txt" }), &ctx(dir.path()))
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("Not a directory"), "{}", out.content);
        assert!(out.content.contains("use `read`"), "{}", out.content);
    }

    #[tokio::test]
    async fn ignore_globs_drop_matching_names() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("keep.rs"), "x").expect("write");
        std::fs::write(dir.path().join("skip.tmp"), "x").expect("write");

        let out = ListTool::new()
            .execute(json!({ "ignore": ["*.tmp"] }), &ctx(dir.path()))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("keep.rs"), "{}", out.content);
        assert!(!out.content.contains("skip.tmp"), "{}", out.content);
        assert!(
            out.content.contains("0 directories, 1 files"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn max_entries_truncates_and_notes_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        for i in 0..5 {
            std::fs::write(dir.path().join(format!("f{i}.txt")), "x").expect("write");
        }
        let out = ListTool::new()
            .execute(json!({ "max_entries": 2 }), &ctx(dir.path()))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content
                .contains("(showing first 2 — raise max_entries or narrow path)"),
            "{}",
            out.content
        );
        assert!(
            out.content.contains("0 directories, 5 files"),
            "totals count everything: {}",
            out.content
        );
        let listed = out.content.matches(".txt").count();
        assert_eq!(listed, 2, "{}", out.content);
    }

    #[tokio::test]
    async fn recursive_walk_lists_nested_and_skips_heavy_dirs() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        std::fs::create_dir_all(dir.path().join("target/debug")).expect("mkdir");
        std::fs::write(dir.path().join("src/main.rs"), "fn main(){}").expect("write");
        std::fs::write(dir.path().join("target/debug/foo.o"), "bin").expect("write");

        let out = ListTool::new()
            .execute(
                json!({ "recursive": true, "max_depth": 5 }),
                &ctx(dir.path()),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("src"), "{}", out.content);
        assert!(out.content.contains("src/main.rs"), "{}", out.content);
        // Heavy dir itself may appear, but its children are pruned.
        assert!(
            !out.content.contains("foo.o"),
            "target/ contents pruned: {}",
            out.content
        );
        assert!(
            out.content.contains("[recursive depth≤5]"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn recursive_max_depth_stops_descent() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("a/b")).expect("mkdir");
        std::fs::write(dir.path().join("a/b/c.txt"), "x").expect("write");

        let shallow = ListTool::new()
            .execute(
                json!({ "recursive": true, "max_depth": 1 }),
                &ctx(dir.path()),
            )
            .await;
        assert!(!shallow.is_error, "{}", shallow.content);
        assert!(shallow.content.contains('a'), "{}", shallow.content);
        assert!(
            !shallow.content.contains("c.txt"),
            "depth 1 must not reach a/b/c.txt: {}",
            shallow.content
        );

        let deep = ListTool::new()
            .execute(
                json!({ "recursive": true, "max_depth": 3 }),
                &ctx(dir.path()),
            )
            .await;
        assert!(!deep.is_error, "{}", deep.content);
        assert!(deep.content.contains("a/b/c.txt"), "{}", deep.content);
    }

    #[tokio::test]
    async fn recursive_max_entries_truncates() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("d")).expect("mkdir");
        for i in 0..4 {
            std::fs::write(dir.path().join(format!("d/f{i}.txt")), "x").expect("write");
        }
        let out = ListTool::new()
            .execute(
                json!({ "recursive": true, "max_depth": 5, "max_entries": 2 }),
                &ctx(dir.path()),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("showing first 2"), "{}", out.content);
        assert!(
            out.content.contains("[recursive depth≤5]"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn recursive_ignore_globs_apply_per_level() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("src")).expect("mkdir");
        std::fs::write(dir.path().join("src/keep.rs"), "x").expect("write");
        std::fs::write(dir.path().join("src/skip.tmp"), "x").expect("write");

        let out = ListTool::new()
            .execute(
                json!({ "recursive": true, "ignore": ["*.tmp"], "max_depth": 3 }),
                &ctx(dir.path()),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("keep.rs"), "{}", out.content);
        assert!(!out.content.contains("skip.tmp"), "{}", out.content);
    }

    #[tokio::test]
    async fn relative_path_resolves_from_working_dir() {
        let root = tempfile::tempdir().expect("tempdir");
        let nested = root.path().join("nested");
        std::fs::create_dir(&nested).expect("mkdir");
        std::fs::write(nested.join("b.txt"), "hi").expect("write");

        let out = ListTool::new()
            .execute(json!({ "path": "nested" }), &ctx(root.path()))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("b.txt"), "{}", out.content);
        assert!(
            out.content.contains("nested"),
            "shown path relative: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn clamps_max_depth_and_max_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), "x").expect("write");

        // 0 clamps to 1 — still lists the single file.
        let out = ListTool::new()
            .execute(json!({ "max_entries": 0 }), &ctx(dir.path()))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("a.txt"), "{}", out.content);

        // Huge max_depth is clamped (20) and still tagged as recursive.
        let out = ListTool::new()
            .execute(
                json!({ "recursive": true, "max_depth": 99 }),
                &ctx(dir.path()),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content.contains("[recursive depth≤20]"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn recursive_uses_warm_index_when_available() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        std::fs::write(dir.path().join("src/lib.rs"), "// lib").expect("write");
        std::fs::write(dir.path().join("README.md"), "hi").expect("write");

        let out = ListTool::new()
            .execute(
                json!({ "recursive": true, "max_depth": 3 }),
                &ctx_with_index(dir.path()),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("README.md"), "{}", out.content);
        assert!(out.content.contains("src/lib.rs"), "{}", out.content);
        assert!(
            out.content.contains("[recursive depth≤3]"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn index_path_honours_ignore_depth_and_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("a/b")).expect("mkdir");
        std::fs::write(dir.path().join("keep.rs"), "x").expect("write");
        std::fs::write(dir.path().join("skip.tmp"), "x").expect("write");
        std::fs::write(dir.path().join("a/b/deep.rs"), "x").expect("write");

        let ctx = ctx_with_index(dir.path());

        let ignored = ListTool::new()
            .execute(
                json!({ "recursive": true, "ignore": ["*.tmp"], "max_depth": 5 }),
                &ctx,
            )
            .await;
        assert!(!ignored.is_error, "{}", ignored.content);
        assert!(ignored.content.contains("keep.rs"), "{}", ignored.content);
        assert!(!ignored.content.contains("skip.tmp"), "{}", ignored.content);

        let shallow = ListTool::new()
            .execute(json!({ "recursive": true, "max_depth": 1 }), &ctx)
            .await;
        assert!(!shallow.is_error, "{}", shallow.content);
        assert!(
            !shallow.content.contains("deep.rs"),
            "depth 1 must skip a/b/deep.rs: {}",
            shallow.content
        );

        let capped = ListTool::new()
            .execute(
                json!({ "recursive": true, "max_depth": 5, "max_entries": 1 }),
                &ctx,
            )
            .await;
        assert!(!capped.is_error, "{}", capped.content);
        assert!(
            capped.content.contains("showing first 1"),
            "{}",
            capped.content
        );
    }

    #[tokio::test]
    async fn index_outside_scope_falls_back_to_walk() {
        let indexed = tempfile::tempdir().expect("tempdir");
        let listed = tempfile::tempdir().expect("tempdir");
        std::fs::write(listed.path().join("only-walk.txt"), "x").expect("write");

        let mut c = ctx_with_index(indexed.path());
        c.working_dir = listed.path().to_string_lossy().into_owned();

        let out = ListTool::new()
            .execute(json!({ "recursive": true }), &c)
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content.contains("only-walk.txt"),
            "walk fallback: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn malformed_ignore_entries_are_skipped() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), "x").expect("write");
        let out = ListTool::new()
            .execute(json!({ "ignore": [1, true, "a.txt"] }), &ctx(dir.path()))
            .await;
        assert!(!out.is_error, "{}", out.content);
        // Only the string glob is honoured; a.txt is ignored.
        assert!(!out.content.contains("a.txt"), "{}", out.content);
        assert!(out.content.contains("(empty)"), "{}", out.content);
    }
}
