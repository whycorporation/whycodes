use serde_json::json;
use std::fs;
use std::io::{BufRead, BufReader, Read as _};
use std::path::Path;

use super::paths::{
    BINARY_SNIFF_LEN, MAX_FULL_READ_BYTES, display_path, file_len, is_binary_bytes, resolve_path,
    suggest_similar,
};
use crate::tool::{Tool, ToolContext};
use whycodes_core::types::ToolResult;

/// Default max lines returned when the model omits `limit`.
const DEFAULT_LIMIT: usize = 400;
/// Hard ceiling so a single read cannot blow the context window.
const HARD_LIMIT: usize = 2000;
/// Most declaration lines one outline shows.
const MAX_OUTLINE_ENTRIES: usize = 400;

/// Whether a disk read may answer with a declaration outline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutlineMode {
    /// Page as usual (`outline: false`, or an explicit `offset`/`limit`).
    Off,
    /// Outline a source file longer than one default page.
    Auto,
    /// Outline any source file repomap can map (`outline: true`).
    Forced,
}

/// The outline mode a set of `read` arguments asks for.
pub(crate) fn outline_mode(args: &serde_json::Value) -> OutlineMode {
    match args["outline"].as_bool() {
        Some(true) => OutlineMode::Forced,
        Some(false) => OutlineMode::Off,
        None if args["offset"].is_null() && args["limit"].is_null() => OutlineMode::Auto,
        None => OutlineMode::Off,
    }
}

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
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read a text file (line-numbered; a long source file returns its declaration outline \
         first) or an internal URL: skill://, agent://, proc://, \
         rule://, pr://, issue://, mcp:// (a bare scheme lists). Long notes: `read agent://tools/read`."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path (relative to project root or absolute), or an internal URL: skill://<name>, agent://<id>, proc://<job-id>, rule://<name>, pr://<number>[/diff], issue://<number>, mcp://<server>/<uri>"
                },
                "offset": {
                    "type": "integer",
                    "description": "1-based line to start from (default: 1)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max lines to return (default: 400, max: 2000)"
                },
                "outline": {
                    "type": "boolean",
                    "description": "Declaration outline instead of lines. Default: on for source files over 400 lines read without offset/limit; false forces lines, true outlines any size"
                }
            },
            "required": ["path"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> whycodes_core::ToolFuture<'a> {
        Box::pin(async move {
            let path_str = args["path"].as_str().unwrap_or("").trim().to_string();
            if path_str.is_empty() {
                return err("Missing required parameter `path`.");
            }

            match super::internal::read_internal_async(&path_str, ctx).await {
                Some(internal) => return internal,
                None => note_disk_read(),
            }

            let offset = args["offset"].as_u64().unwrap_or(1).max(1) as usize;
            let limit = args["limit"]
                .as_u64()
                .map(|n| n as usize)
                .unwrap_or(DEFAULT_LIMIT)
                .clamp(1, HARD_LIMIT);
            let mode = outline_mode(&args);
            let working_dir = ctx.working_dir.clone();
            let ctx_clone = ctx.clone();
            crate::blocking::tool(Box::new(move || {
                Self::run(path_str, offset, limit, mode, working_dir, ctx_clone)
            }))
            .await
        })
    }
}

impl ReadTool {
    fn run(
        path_str: String,
        offset: usize,
        limit: usize,
        mode: OutlineMode,
        working_dir: String,
        ctx: ToolContext,
    ) -> ToolResult {
        let full_path = resolve_path(&working_dir, &path_str);
        let shown = display_path(&full_path, &working_dir);

        if !full_path.exists() {
            let mut msg = format!("File not found: {}", shown);
            let suggestions = suggest_similar(&full_path, 5);
            if !suggestions.is_empty() {
                msg.push_str("\nDid you mean: ");
                msg.push_str(&suggestions.join(", "));
                msg.push('?');
            }
            return err(&msg);
        }

        if full_path.is_dir() {
            return err(&format!(
                "'{}' is a directory. Use the `list` tool instead of `read`.",
                shown
            ));
        }

        // Multimodal: small images become a structured payload the session layer
        // turns into ContentBlock::Image for vision models (A6).
        match image_media_type(&full_path) {
            Some(media) => return image_read(&full_path, &shown, media),
            None => note_text_read(),
        }

        // Binary sniff without loading the whole file.
        if refuse_binary(&full_path, &shown) {
            return binary_refused(&shown, file_len(&full_path).unwrap_or(0));
        }

        let size = file_len(&full_path).unwrap_or(0);
        let stale = ctx
            .check_file_read(&full_path)
            .map(|s| s.writer_label.clone());
        match super::notebook::Notebook::read(&full_path) {
            Some(nb) => {
                let window = text_window(&nb.rendering, offset, limit);
                let header = format!("{shown} (notebook: cells as text, outputs summarized)");
                return window_from(Ok(window), &header, size, limit, stale);
            }
            None => note_text_read(),
        }
        note_large_default_window(size, offset, limit);

        match outline_read(&full_path, &shown, size, mode) {
            Some(mut outline) => {
                note_stale(&mut outline, &shown, stale.as_deref());
                return ok(&outline);
            }
            None => note_paged_text(),
        }

        window_from(
            take_read_window(read_window(&full_path, offset, limit).map_err(io_error_string)),
            &shown,
            size,
            limit,
            stale,
        )
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

/// The `offset`/`limit` window of an in-memory text (a notebook rendering).
fn text_window(text: &str, offset: usize, limit: usize) -> ReadWindow {
    let all: Vec<&str> = text.lines().collect();
    let lines: Vec<String> = all
        .iter()
        .skip(offset - 1)
        .take(limit)
        .map(|line| line.to_string())
        .collect();
    let end_line = offset - 1 + lines.len();
    ReadWindow {
        start_line: offset,
        end_line,
        total_lines: all.len(),
        total_known: true,
        truncated: end_line < all.len(),
        lines,
    }
}

fn truncate_line(line: &str, max_chars: usize) -> String {
    if line.chars().count() <= max_chars {
        return line.to_string();
    }
    let t: String = line.chars().take(max_chars).collect();
    format!("{t}…[line truncated]")
}

fn image_bytes_result(
    shown: &str,
    media: &str,
    size: u64,
    result: Result<Vec<u8>, String>,
) -> ToolResult {
    match result {
        Ok(bytes) => {
            use base64::Engine as _;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            ok(&format!(
                "Image file `{shown}` ({media}, {}).\n\
                 WHYCODES_IMAGE_B64:{media}\n{b64}",
                super::paths::human_size(size)
            ))
        }
        Err(e) => image_read_error(shown, &e),
    }
}

fn take_read_window(result: Result<ReadWindow, String>) -> Result<ReadWindow, String> {
    result
}

fn window_from(
    result: Result<ReadWindow, String>,
    shown: &str,
    size: u64,
    limit: usize,
    stale_writer: Option<String>,
) -> ToolResult {
    match result {
        Err(e) => window_read_error(shown, &e),
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
            let refs: Vec<&str> = window.lines.iter().map(String::as_str).collect();
            let tags = super::line_tag::tags_for_window(&refs);
            for (i, line) in window.lines.iter().enumerate() {
                let n = window.start_line + i;
                // Cap absurdly long lines to protect context
                let line = truncate_line(line, 4000);
                out.push_str(&super::line_tag::format_read_line(n, &tags[i], &line));
                out.push('\n');
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
            note_stale(&mut out, shown, stale_writer.as_deref());
            ToolResult {
                tool_call_id: String::new(),
                content: out,
                is_error: false,
            }
        }
    }
}

/// Declaration outline of a source file, or `None` to page it as usual.
fn outline_read(path: &Path, shown: &str, size: u64, mode: OutlineMode) -> Option<String> {
    let forced = match mode {
        OutlineMode::Off => return None,
        OutlineMode::Auto => false,
        OutlineMode::Forced => true,
    };
    // A `?` here is an expansion line coverage misses (see `image_media_type`).
    #[allow(clippy::question_mark)]
    let (total, entries) = match super::repomap::file_outline(path) {
        Some(outline) => outline,
        None => return None,
    };
    if entries.is_empty() || (!forced && total <= DEFAULT_LIMIT) {
        return None;
    }
    Some(render_outline(shown, size, total, &entries))
}

/// Declaration lines keep their indentation (nesting) and get the same
/// content tags a normal read would, so `edit` can name them directly.
fn render_outline(shown: &str, size: u64, total: usize, entries: &[(usize, String)]) -> String {
    let kept = &entries[..entries.len().min(MAX_OUTLINE_ENTRIES)];
    let mut out = format!(
        "# {shown}\n# outline: {} declarations of {total} lines  |  {}\n",
        entries.len(),
        super::paths::human_size(size)
    );
    let refs: Vec<&str> = kept.iter().map(|(_, line)| line.as_str()).collect();
    let tags = super::line_tag::tags_for_window(&refs);
    for ((n, line), tag) in kept.iter().zip(&tags) {
        out.push_str(&super::line_tag::format_read_line(
            *n,
            tag,
            &truncate_line(line, 4000),
        ));
        out.push('\n');
    }
    if kept.len() < entries.len() {
        out.push_str(&format!(
            "[outline shows the first {} of {} declarations]\n",
            kept.len(),
            entries.len()
        ));
    }
    out.push_str(&format!(
        "\n[file has {total} lines; showing its outline. Read a range with offset/limit, \
         e.g. offset={} limit=80]",
        kept[0].0
    ));
    out
}

fn note_stale(out: &mut String, shown: &str, stale_writer: Option<&str>) {
    match stale_writer {
        Some(writer_label) => note_stale_read(out, shown, writer_label),
        None => note_fresh_read(),
    }
}

fn note_paged_text() {}

fn note_disk_read() {}

fn note_text_read() {}

fn note_fresh_read() {}

fn note_stale_read(out: &mut String, shown: &str, writer_label: &str) {
    out.push_str(&format!(
        "\n[stale] `{shown}` was written by swarm agent `{writer_label}` since your last read.",
    ));
}

fn image_read(path: &Path, shown: &str, media: &str) -> ToolResult {
    let size = file_len(path).unwrap_or(0);
    const MAX_IMAGE: u64 = 2 * 1024 * 1024;
    if size == 0 || size > MAX_IMAGE {
        return err(&format!(
            "'{}' is an image ({media}, {}) — max {} for vision read. \
             Attach with @path in the TUI for larger files.",
            shown,
            super::paths::human_size(size),
            super::paths::human_size(MAX_IMAGE)
        ));
    }
    image_bytes_result(shown, media, size, fs::read(path).map_err(io_error_string))
}

fn io_error_string(e: std::io::Error) -> String {
    e.to_string()
}

fn image_read_error(shown: &str, e: &str) -> ToolResult {
    err(&format!("Failed to read image `{shown}`: {e}"))
}

fn window_read_error(shown: &str, e: &str) -> ToolResult {
    err(&format!("Error reading '{shown}': {e}"))
}

fn note_large_default_window(size: u64, offset: usize, limit: usize) {
    match (
        size > MAX_FULL_READ_BYTES,
        offset == 1,
        limit >= DEFAULT_LIMIT,
    ) {
        (true, true, true) => note_paged_read(),
        _ => note_small_window(),
    }
}

fn note_paged_read() {}

fn note_small_window() {}

fn refuse_binary(path: &Path, shown: &str) -> bool {
    let _ = shown;
    sniff_opened(fs::File::open(path))
}

fn sniff_opened(file: std::io::Result<fs::File>) -> bool {
    match file {
        Ok(mut f) => sniff_head(&mut f),
        Err(_open) => false,
    }
}

fn sniff_head(file: &mut fs::File) -> bool {
    let mut head = [0u8; BINARY_SNIFF_LEN];
    sniff_read(file.read(&mut head), &head)
}

fn sniff_read(result: std::io::Result<usize>, head: &[u8]) -> bool {
    match result {
        Ok(n) => is_binary_bytes(&head[..n.min(head.len())]),
        Err(_e) => false,
    }
}

fn binary_refused(shown: &str, size: u64) -> ToolResult {
    err(&format!(
        "'{}' looks like a binary file ({}). Refusing to dump raw bytes into context.",
        shown,
        super::paths::human_size(size)
    ))
}

fn ok(msg: &str) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: msg.to_string(),
        is_error: false,
    }
}

fn err(msg: &str) -> ToolResult {
    ToolResult {
        tool_call_id: String::new(),
        content: msg.to_string(),
        is_error: true,
    }
}

fn image_media_type(path: &Path) -> Option<&'static str> {
    // `?` on `Option` is an expansion `-skip-expansions` counts as its own
    // line and then misses. A one-line `match` stays in the function body.
    #[allow(clippy::question_mark)]
    let ext = match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) => ext.to_ascii_lowercase(),
        None => return None,
    };
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        _ => None,
    }
}

#[cfg(test)]
#[path = "read_tests.rs"]
mod tests;
