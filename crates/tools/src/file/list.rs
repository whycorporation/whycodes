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
