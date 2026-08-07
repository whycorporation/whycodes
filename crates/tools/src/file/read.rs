use async_trait::async_trait;
use serde_json::json;
use std::fs;
use std::io::{BufRead, BufReader, Read as _};
use std::path::Path;

use super::paths::{
    BINARY_SNIFF_LEN, MAX_FULL_READ_BYTES, display_path, file_len, is_binary_bytes, resolve_path,
    suggest_similar,
};
use crate::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

/// Default max lines returned when the model omits `limit`.
const DEFAULT_LIMIT: usize = 400;
/// Hard ceiling so a single read cannot blow the context window.
const HARD_LIMIT: usize = 2000;

pub struct ReadTool;

impl Default for ReadTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ReadTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read a text file (line-numbered). Prefer project-relative paths. \
         Use offset/limit for large files instead of reading everything. \
         For directories use `list`; for finding files use `glob`/`grep`. \
         Do not read cargo registry, node_modules, or target/ artifacts."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path (relative to project root or absolute)"
                },
                "offset": {
                    "type": "integer",
                    "description": "1-based line to start from (default: 1)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max lines to return (default: 400, max: 2000)"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let path_str = args["path"].as_str().unwrap_or("").trim();
        if path_str.is_empty() {
            return err("Missing required parameter `path`.");
        }

        let offset = args["offset"].as_u64().unwrap_or(1).max(1) as usize;
        let limit = args["limit"]
            .as_u64()
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_LIMIT)
            .clamp(1, HARD_LIMIT);

        let full_path = resolve_path(&ctx.working_dir, path_str);
        let shown = display_path(&full_path, &ctx.working_dir);

        if !full_path.exists() {
            let mut msg = format!("File not found: {}", shown);
            let suggestions = suggest_similar(&full_path, 5);
            if !suggestions.is_empty() {
                msg.push_str("\nDid you mean: ");
                msg.push_str(&suggestions.join(", "));
                msg.push('?');
            }
            return err(msg);
        }

        if full_path.is_dir() {
            return err(format!(
                "'{}' is a directory. Use the `list` tool instead of `read`.",
                shown
            ));
        }

        // Multimodal: small images become a structured payload the session layer
        // turns into ContentBlock::Image for vision models (A6).
        if let Some(media) = image_media_type(&full_path) {
            let size = file_len(&full_path).unwrap_or(0);
            const MAX_IMAGE: u64 = 2 * 1024 * 1024;
            if size == 0 || size > MAX_IMAGE {
                return err(format!(
                    "'{}' is an image ({media}, {}) — max {} for vision read. \
                     Attach with @path in the TUI for larger files.",
                    shown,
                    super::paths::human_size(size),
                    super::paths::human_size(MAX_IMAGE)
                ));
            }
            match fs::read(&full_path) {
                Ok(bytes) => {
                    use base64::Engine as _;
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                    return ok(format!(
                        "Image file `{shown}` ({media}, {}).\n\
                         WHYCODE_IMAGE_B64:{media}\n{b64}",
                        super::paths::human_size(size)
                    ));
                }
                Err(e) => return err(format!("Failed to read image `{shown}`: {e}")),
            }
        }

        // Binary sniff without loading the whole file.
        if let Ok(mut f) = fs::File::open(&full_path) {
            let mut head = [0u8; BINARY_SNIFF_LEN];
            if let Ok(n) = f.read(&mut head)
                && is_binary_bytes(&head[..n])
            {
                let size = file_len(&full_path).unwrap_or(0);
                return err(format!(
                    "'{}' looks like a binary file ({}). Refusing to dump raw bytes into context.",
                    shown,
                    super::paths::human_size(size)
                ));
            }
        }

        let size = file_len(&full_path).unwrap_or(0);
        if size > MAX_FULL_READ_BYTES && offset == 1 && limit >= DEFAULT_LIMIT {
            // Still allow windowed reads of huge files — stream below.
            // Warn when the default window is used so the model knows to page.
        }

        match read_window(&full_path, offset, limit) {
            Ok(window) => {
                let mut out = String::with_capacity(
                    window.lines.iter().map(|l| l.len() + 12).sum::<usize>() + 128,
                );
                out.push_str(&format!(
                    "# {}\n# lines {}–{} of {}  |  {}\n",
                    shown,
                    window.start_line,
                    window.end_line,
                    if window.total_known {
                        window.total_lines.to_string()
                    } else {
                        format!("≥{}", window.total_lines)
                    },
                    super::paths::human_size(size)
                ));
                for (i, line) in window.lines.iter().enumerate() {
                    let n = window.start_line + i;
                    // Cap absurdly long lines to protect context
                    let line = truncate_line(line, 4000);
                    out.push_str(&format!("{:6}|{}\n", n, line));
                }
                if window.truncated {
                    out.push_str(&format!(
                        "\n[truncated — showing {} lines starting at {}. \
                         Re-call with offset={} limit={} for more.]",
                        window.lines.len(),
                        window.start_line,
                        window.end_line + 1,
                        limit
                    ));
                }
                ToolResult {
                    tool_call_id: String::new(),
                    content: out,
                    is_error: false,
                }
            }
            Err(e) => err(format!("Error reading '{}': {}", shown, e)),
        }
    }
}

struct ReadWindow {
    lines: Vec<String>,
    start_line: usize,
    end_line: usize,
    total_lines: usize,
    total_known: bool,
    truncated: bool,
}

/// Stream-read only the requested line window (plus a cheap total when small).
fn read_window(path: &Path, offset: usize, limit: usize) -> std::io::Result<ReadWindow> {
    let file = fs::File::open(path)?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut reader = BufReader::with_capacity(64 * 1024, file);

    let mut line_no = 0usize;
    let mut lines = Vec::with_capacity(limit.min(512));
    let mut buf = String::with_capacity(256);

    // Skip until offset
    while line_no + 1 < offset {
        buf.clear();
        let n = reader.read_line(&mut buf)?;
        if n == 0 {
            break;
        }
        line_no += 1;
    }

    // Collect up to `limit` lines
    while lines.len() < limit {
        buf.clear();
        let n = reader.read_line(&mut buf)?;
        if n == 0 {
            break;
        }
        line_no += 1;
        // strip trailing newline kept by read_line
        if buf.ends_with('\n') {
            buf.pop();
            if buf.ends_with('\r') {
                buf.pop();
            }
        }
        lines.push(std::mem::take(&mut buf));
    }

    let start_line = if lines.is_empty() {
        offset
    } else {
        line_no - lines.len() + 1
    };
    let end_line = if lines.is_empty() {
        start_line.saturating_sub(1)
    } else {
        start_line + lines.len() - 1
    };

    // Peek one more line to know if truncated
    buf.clear();
    let more = reader.read_line(&mut buf)? > 0;
    let mut total_lines = line_no + if more { 1 } else { 0 };
    let mut total_known = !more;

    // For modest files, finish counting so the header is exact.
    // Skip full scan on large files to stay fast.
    if more && size <= MAX_FULL_READ_BYTES {
        loop {
            buf.clear();
            let n = reader.read_line(&mut buf)?;
            if n == 0 {
                break;
            }
            total_lines += 1;
        }
        total_known = true;
    } else if more {
        // Keep streaming just enough to estimate? Stay cheap: report ≥.
        total_known = false;
    }

    Ok(ReadWindow {
        lines,
        start_line,
        end_line,
        total_lines,
        total_known,
        truncated: more || (total_known && end_line < total_lines),
    })
}

fn truncate_line(line: &str, max_chars: usize) -> String {
    if line.chars().count() <= max_chars {
        return line.to_string();
    }
    let t: String = line.chars().take(max_chars).collect();
    format!("{t}…[line truncated]")
}

fn ok(msg: impl Into<String>) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: msg.into(),
        is_error: false,
    }
}

fn err(msg: impl Into<String>) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: msg.into(),
        is_error: true,
    }
}

fn image_media_type(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        _ => None,
    }
}
