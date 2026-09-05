//! Budget-capped signature map of a tree (Bonsai-style skeleton, no dump).
//!
//! Walks source files (index when warm), keeps declaration-like lines, packs
//! until `max_tokens`. Prefer this over a first-turn `glob` + `read` loop.

use serde_json::json;
use std::path::{Path, PathBuf};

use super::paths::{display_path, file_len, is_binary_file, resolve_path, visit_index, walk_files};
use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

const DEFAULT_MAX_TOKENS: usize = 4_000;
const HARD_MAX_TOKENS: usize = 16_000;
const MIN_TOKENS: usize = 500;
const DEFAULT_MAX_FILES: usize = 80;
const HARD_MAX_FILES: usize = 400;
const MAX_FILE_BYTES: u64 = 512 * 1024;
const MAX_SIGS_PER_FILE: usize = 40;
const MAX_SIG_CHARS: usize = 120;

const SOURCE_EXT: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "java", "kt", "kts", "c", "h", "cc",
    "cpp", "hpp", "cs", "rb", "php", "swift", "vue", "svelte", "toml", "md",
];

#[derive(Default)]
pub struct RepoMapTool;

impl RepoMapTool {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for RepoMapTool {
    fn name(&self) -> &str {
        "repomap"
    }

    fn description(&self) -> &str {
        "Signature map of a directory: declaration-like lines (fn/struct/class/impl), \
         packed to a token budget. Use to orient before `read`. Not a full dump — \
         bodies are omitted. Served from the live file index when warm."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory or file to map (default: project root)"
                },
                "max_tokens": {
                    "type": "integer",
                    "description": "Approximate token budget for the whole map (default: 4000, max: 16000)"
                },
                "max_files": {
                    "type": "integer",
                    "description": "Max source files to inspect (default: 80)"
                }
            }
        })
    }

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            let working_dir = ctx.working_dir.clone();
            let file_index = ctx.file_index.clone();
            crate::blocking::tool(move || Self::run(args, working_dir, file_index)).await
        })
    }
}

impl RepoMapTool {
    fn run(
        args: serde_json::Value,
        working_dir: String,
        file_index: Option<std::sync::Arc<whycodes_index::WorkspaceIndex>>,
    ) -> ToolResult {
        let root_arg = args["path"].as_str().unwrap_or(".");
        let root = resolve_path(&working_dir, root_arg);
        let root_shown = display_path(&root, &working_dir);

        if !root.exists() {
            return ToolResult {
                tool_call_id: String::new(),
                content: format!("Path not found: {root_shown}"),
                is_error: true,
            };
        }

        let max_tokens = args["max_tokens"]
            .as_u64()
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_MAX_TOKENS)
            .clamp(MIN_TOKENS, HARD_MAX_TOKENS);
        let max_files = args["max_files"]
            .as_u64()
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_MAX_FILES)
            .clamp(1, HARD_MAX_FILES);

        let mut files: Vec<(PathBuf, String)> = Vec::new();
        let mut collect = |path: &Path, rel: &str| -> bool {
            if !is_source_path(path) {
                return true;
            }
            files.push((path.to_path_buf(), rel.replace('\\', "/")));
            files.len() < max_files
        };

        if root.is_file() {
            let rel = display_path(&root, &working_dir);
            collect(&root, &rel);
        } else {
            let used_index = if let Some(idx) = file_index.as_deref() {
                visit_index(idx, &root, &mut |path, rel, is_dir, _size| {
                    if is_dir {
                        return true;
                    }
                    collect(path, rel)
                })
                .is_some()
            } else {
                false
            };
            if !used_index {
                walk_files(&root, &mut |path, rel| collect(path, rel));
            }
        }

        files.sort_by(|a, b| rank_key(&a.1).cmp(&rank_key(&b.1)));

        let inspected = files.len();
        let mut blocks: Vec<FileBlock> = Vec::new();
        for (path, rel) in files {
            if file_len(&path).is_some_and(|n| n > MAX_FILE_BYTES) {
                continue;
            }
            if is_binary_file(&path) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let sigs = extract_signatures(&text, &ext);
            if sigs.is_empty() {
                continue;
            }
            blocks.push(FileBlock { rel, sigs });
        }

        let (body, used_files, omitted) = pack_blocks(&blocks, max_tokens);
        let used_tokens = estimate_tokens(&body);
        let header = format!(
            "# repomap  {root_shown}  ~{used_tokens}/{max_tokens} tokens  \
             {used_files} files  {omitted} omitted  ({inspected} inspected)"
        );
        let content = if body.is_empty() {
            format!("{header}\n(no signatures in scope)")
        } else {
            format!("{header}\n{body}")
        };

        ToolResult {
            tool_call_id: String::new(),
            content,
            is_error: false,
        }
    }
}

struct FileBlock {
    rel: String,
    sigs: Vec<String>,
}

fn pack_blocks(blocks: &[FileBlock], max_tokens: usize) -> (String, usize, usize) {
    let mut out = String::new();
    let mut used = 0usize;
    let header_reserve = 80;
    let budget_chars = max_tokens.saturating_mul(4).saturating_sub(header_reserve);

    for (i, block) in blocks.iter().enumerate() {
        let mut chunk = format!("{}\n", block.rel);
        for sig in &block.sigs {
            chunk.push_str("  ");
            chunk.push_str(sig);
            chunk.push('\n');
        }
        if out.len() + chunk.len() > budget_chars && !out.is_empty() {
            return (out, used, blocks.len() - i);
        }
        out.push_str(&chunk);
        used += 1;
    }
    (out, used, 0)
}

fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

fn is_source_path(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
        return false;
    };
    let ext = ext.to_ascii_lowercase();
    SOURCE_EXT.iter().any(|e| *e == ext)
}

/// Lower rank is packed first: manifests, then `src/`, then everything else, tests last.
fn rank_key(rel: &str) -> (u8, &str) {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let bucket = if matches!(
        name,
        "Cargo.toml"
            | "package.json"
            | "pyproject.toml"
            | "go.mod"
            | "lib.rs"
            | "mod.rs"
            | "main.rs"
    ) {
        0
    } else if rel.contains("/src/") || rel.starts_with("src/") {
        1
    } else if rel.contains("/tests/")
        || rel.starts_with("tests/")
        || rel.contains("_test.")
        || rel.ends_with("_test.rs")
    {
        3
    } else {
        2
    };
    (bucket, rel)
}

fn extract_signatures(text: &str, ext: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        if out.len() >= MAX_SIGS_PER_FILE {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || (trimmed.starts_with('#') && ext != "md")
        {
            continue;
        }
        if trimmed.starts_with("/*") || trimmed.starts_with("*") {
            continue;
        }
        if is_signature_line(trimmed, ext) {
            out.push(truncate_sig(trimmed));
        }
    }
    out
}

fn is_signature_line(line: &str, ext: &str) -> bool {
    match ext {
        "rs" => rust_sig(line),
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "vue" | "svelte" => js_sig(line),
        "py" => py_sig(line),
        "go" => {
            line.starts_with("func ") || line.starts_with("type ") || line.starts_with("package ")
        }
        "java" | "kt" | "kts" | "cs" | "swift" => {
            line.contains(" class ")
                || line.contains(" interface ")
                || line.contains(" enum ")
                || line.starts_with("class ")
                || line.starts_with("interface ")
                || line.starts_with("enum ")
                || line.starts_with("fun ")
                || line.starts_with("func ")
                || line.starts_with("public ")
                || line.starts_with("export ")
        }
        "c" | "h" | "cc" | "cpp" | "hpp" => {
            !line.ends_with(';')
                && (line.contains('(') && !line.starts_with("if ") && !line.starts_with("while "))
                && (line.starts_with("struct ")
                    || line.starts_with("class ")
                    || line.starts_with("enum ")
                    || line.starts_with("typedef ")
                    || line.starts_with("namespace "))
        }
        "toml" => line.starts_with('[') && line.ends_with(']'),
        "md" => {
            let hashes = line.bytes().take_while(|b| *b == b'#').count();
            hashes > 0 && hashes <= 3 && line.as_bytes().get(hashes) == Some(&b' ')
        }
        _ => false,
    }
}

fn rust_sig(line: &str) -> bool {
    let s = line
        .trim_start_matches("pub(crate) ")
        .trim_start_matches("pub ");
    s.starts_with("fn ")
        || s.starts_with("async fn ")
        || s.starts_with("struct ")
        || s.starts_with("enum ")
        || s.starts_with("trait ")
        || s.starts_with("impl ")
        || s.starts_with("mod ")
        || s.starts_with("type ")
        || s.starts_with("const ")
        || s.starts_with("static ")
        || line.starts_with("impl ")
}

fn js_sig(line: &str) -> bool {
    let s = line
        .trim_start_matches("export ")
        .trim_start_matches("default ");
    s.starts_with("function ")
        || s.starts_with("async function ")
        || s.starts_with("class ")
        || s.starts_with("interface ")
        || s.starts_with("type ")
        || s.starts_with("enum ")
        || (s.starts_with("const ") && (s.contains(" = (") || s.contains(" = function")))
}

fn py_sig(line: &str) -> bool {
    line.starts_with("def ") || line.starts_with("async def ") || line.starts_with("class ")
}

fn truncate_sig(line: &str) -> String {
    let mut s = line.to_string();
    if let Some(idx) = s.find(" {") {
        s.truncate(idx);
    }
    s = s.trim_end_matches('{').trim_end().to_string();
    if s.chars().count() > MAX_SIG_CHARS {
        let mut cut = s
            .chars()
            .take(MAX_SIG_CHARS.saturating_sub(1))
            .collect::<String>();
        cut.push('…');
        cut
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolContext;

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir.to_string_lossy().into_owned())
    }

    fn write(dir: &std::path::Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(path, body).expect("write");
    }

    #[tokio::test]
    async fn metadata_describes_repomap() {
        let t = RepoMapTool::new();
        assert_eq!(t.name(), "repomap");
        assert!(t.description().to_ascii_lowercase().contains("signature"));
        let params = t.parameters();
        assert!(params["properties"]["path"].is_object());
        assert!(params["properties"]["max_tokens"].is_object());
    }

    #[tokio::test]
    async fn missing_root_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = RepoMapTool::new()
            .execute(json!({ "path": "nope" }), &ctx(dir.path()))
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("Path not found"), "{}", out.content);
    }

    #[tokio::test]
    async fn maps_rust_signatures_and_respects_budget() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "src/lib.rs",
            "pub fn greet(name: &str) -> String {\n    format!(\"hi {name}\")\n}\n\
             pub struct Person { pub name: String }\n\
             impl Person {\n    pub fn new(name: String) -> Self { Self { name } }\n}\n",
        );
        write(dir.path(), "src/skip.bin", "not source");
        write(
            dir.path(),
            "tests/extra.rs",
            "fn helper() {}\nfn other() {}\n",
        );

        let out = RepoMapTool::new()
            .execute(json!({ "max_tokens": 2000 }), &ctx(dir.path()))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("# repomap"), "{}", out.content);
        assert!(out.content.contains("src/lib.rs"), "{}", out.content);
        assert!(out.content.contains("fn greet"), "{}", out.content);
        assert!(out.content.contains("struct Person"), "{}", out.content);
        assert!(
            !out.content.contains("format!"),
            "bodies must be omitted: {}",
            out.content
        );
        // src/ ranks ahead of tests/
        let src = out.content.find("src/lib.rs").expect("src");
        if let Some(tests) = out.content.find("tests/extra.rs") {
            assert!(src < tests, "{}", out.content);
        }
    }

    #[test]
    fn pack_blocks_stops_at_budget() {
        let blocks: Vec<FileBlock> = (0..20)
            .map(|i| FileBlock {
                rel: format!("src/f{i}.rs"),
                sigs: vec!["fn quite_a_long_function_name_for_budget()".into(); 12],
            })
            .collect();
        let (body, used, omitted) = pack_blocks(&blocks, 500);
        assert!(!body.is_empty());
        assert!(used < 20, "used={used}");
        assert!(omitted > 0, "omitted={omitted}");
    }

    #[test]
    fn rust_and_js_extractors() {
        let rust = extract_signatures(
            "/// docs\npub fn foo() {\n    1\n}\nimpl Bar {\n    fn baz(&self) {}\n}\n",
            "rs",
        );
        assert!(rust.iter().any(|s| s.contains("fn foo")), "{rust:?}");
        assert!(rust.iter().any(|s| s.contains("impl Bar")), "{rust:?}");
        assert!(rust.iter().all(|s| !s.contains("1")), "{rust:?}");

        let js = extract_signatures(
            "export function ping() { return 1 }\nconst x = 1;\nexport class Box {}\n",
            "ts",
        );
        assert!(js.iter().any(|s| s.contains("function ping")), "{js:?}");
        assert!(js.iter().any(|s| s.contains("class Box")), "{js:?}");
    }

    #[test]
    fn rank_prefers_src_over_tests() {
        assert!(rank_key("src/lib.rs") < rank_key("tests/foo.rs"));
        assert!(rank_key("Cargo.toml") < rank_key("src/lib.rs"));
    }
}
