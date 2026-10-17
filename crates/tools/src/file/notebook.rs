//! Jupyter notebooks (`.ipynb`) as editable text.
//!
//! `read` shows a notebook as its cells: a marker line per cell
//! (`# %% [code] cell:N`), the cell source, and a one-line summary of a code
//! cell's outputs. `edit` and `write` work on that rendering and map it back
//! to JSON: cells whose source did not change are kept byte for byte, a
//! changed code cell loses its outputs and execution count (as when Jupyter
//! edits a cell), a marker without a number adds a cell, and a removed block
//! deletes one. Damaged markers are refused, not guessed.

use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::path::Path;

use super::line_tag::hash_u64;
use super::tolerant;

/// Cell types a marker may name.
const KINDS: [&str; 3] = ["code", "markdown", "raw"];

pub(crate) fn is_notebook_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("ipynb"))
}

/// A parsed notebook and its text rendering.
pub(crate) struct Notebook {
    /// The file as it is on disk; also the formatting model for a rewrite.
    raw: String,
    value: Value,
    cells: Vec<Map<String, Value>>,
    /// A notebook created by `write`: marker numbers have nothing to refer to.
    fresh: bool,
    pub(crate) rendering: String,
}

impl Notebook {
    /// The notebook in `raw`, or `None` when `path` is not a `.ipynb` or the
    /// text is not notebook JSON (it is then handled as plain text).
    pub(crate) fn open(path: &Path, raw: &str) -> Option<Self> {
        if !is_notebook_path(path) {
            return None;
        }
        match serde_json::from_str(raw) {
            Ok(value) => Self::from_value(value, raw.to_string()),
            Err(e) => {
                tracing::debug!(error = %e, "notebook: not JSON, handled as text");
                None
            }
        }
    }

    /// The notebook at `path`, or `None` when it is not one or cannot be read.
    pub(crate) fn read(path: &Path) -> Option<Self> {
        match std::fs::read_to_string(path) {
            Ok(raw) => Self::open(path, &raw),
            Err(e) => {
                tracing::debug!(error = %e, "notebook: unreadable");
                None
            }
        }
    }

    /// An empty nbformat 4.5 notebook, as Jupyter would write it.
    fn fresh() -> Self {
        let value = json!({"cells": [], "metadata": {}, "nbformat": 4, "nbformat_minor": 5});
        let mut raw = String::new();
        pretty(&mut raw, &value, " ", 0);
        raw.push('\n');
        Self {
            raw,
            value,
            cells: Vec::new(),
            fresh: true,
            rendering: String::new(),
        }
    }

    fn from_value(value: Value, raw: String) -> Option<Self> {
        let cells = value["cells"]
            .as_array()?
            .iter()
            .map(|cell| cell.as_object().cloned())
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            rendering: render(&cells),
            raw,
            value,
            cells,
            fresh: false,
        })
    }

    /// The notebook JSON for an edited `text` rendering. Unchanged text gives
    /// back the file exactly as it was.
    pub(crate) fn apply(&self, text: &str) -> Result<String, String> {
        let blocks = split_blocks(text).map_err(damaged)?;
        let mut seen = vec![false; self.cells.len()];
        let mut ids: HashSet<String> = self
            .cells
            .iter()
            .filter_map(|cell| cell.get("id").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        let uses_ids = !ids.is_empty() || self.value["nbformat_minor"].as_u64() >= Some(5);
        let mut cells = Vec::with_capacity(blocks.len());
        for (i, block) in blocks.iter().enumerate() {
            let index = self.cell_index(block, &mut seen).map_err(damaged)?;
            let summary = index.and_then(|j| output_summary(&self.cells[j]));
            let source = source_of(block, i + 1 == blocks.len(), summary.as_deref());
            cells.push(match index {
                Some(j) => updated_cell(&self.cells[j], &block.kind, &source),
                None => new_cell(&block.kind, &source, uses_ids, &mut ids),
            });
        }
        let mut value = self.value.clone();
        value["cells"] = Value::Array(cells);
        if value == self.value {
            return Ok(self.raw.clone());
        }
        Ok(serialize(&value, &self.raw))
    }

    /// The original cell a block's marker names, or `None` for a new cell.
    fn cell_index(&self, block: &Block, seen: &mut [bool]) -> Result<Option<usize>, String> {
        if !KINDS.contains(&block.kind.as_str()) {
            return Err(format!(
                "line {}: unknown cell type `{}` (use code, markdown or raw)",
                block.line, block.kind
            ));
        }
        if self.fresh || block.tag.is_empty() || block.tag == "new" {
            return Ok(None);
        }
        match block.tag.parse::<usize>() {
            Ok(n) if (1..=seen.len()).contains(&n) && !seen[n - 1] => {
                seen[n - 1] = true;
                Ok(Some(n - 1))
            }
            Ok(n) if (1..=seen.len()).contains(&n) => Err(format!(
                "line {}: `cell:{n}` appears more than once",
                block.line
            )),
            _ => Err(format!(
                "line {}: `cell:{}` is not a cell of this notebook (it has {})",
                block.line,
                block.tag,
                seen.len()
            )),
        }
    }
}

/// `write` content for `path`: marker text for a notebook becomes notebook
/// JSON (merged into the existing file when there is one); anything else,
/// raw JSON included, is written as given.
pub(crate) fn convert_write(path: &Path, content: &str) -> Result<String, String> {
    if !is_notebook_path(path) || !looks_like_rendering(content) {
        return Ok(content.to_string());
    }
    Notebook::read(path)
        .unwrap_or_else(Notebook::fresh)
        .apply(content)
}

fn looks_like_rendering(content: &str) -> bool {
    content
        .lines()
        .find(|line| !line.trim().is_empty())
        .and_then(|line| marker(line.trim_end()))
        .is_some()
}

fn damaged(e: String) -> String {
    format!(
        "Notebook cell markers are damaged — {e}. Nothing was written. \
         Re-read the notebook: every cell starts with its own `# %% [code] cell:N` \
         line; mark an added cell `cell:new`."
    )
}

fn cell_kind(cell: &Map<String, Value>) -> &str {
    cell.get("cell_type")
        .and_then(Value::as_str)
        .unwrap_or("raw")
}

fn source_text(cell: &Map<String, Value>) -> String {
    match cell.get("source") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts.iter().filter_map(Value::as_str).collect(),
        _ => String::new(),
    }
}

/// `text` in the shape of `like`: a string stays a string, anything else
/// becomes Jupyter's list of lines.
fn source_value(text: &str, like: Option<&Value>) -> Value {
    match like {
        Some(Value::String(_)) => Value::from(text),
        _ => text.split_inclusive('\n').map(Value::from).collect(),
    }
}

/// `# out: 2 outputs (stream, image/png)` for a code cell with outputs.
fn output_summary(cell: &Map<String, Value>) -> Option<String> {
    let outputs = cell.get("outputs").and_then(Value::as_array)?;
    if outputs.is_empty() {
        return None;
    }
    let mut kinds: Vec<String> = Vec::new();
    for output in outputs {
        for kind in output_kinds(output) {
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
        }
    }
    let plural = if outputs.len() == 1 { "" } else { "s" };
    Some(format!(
        "# out: {} output{plural} ({})",
        outputs.len(),
        kinds.join(", ")
    ))
}

fn output_kinds(output: &Value) -> Vec<String> {
    match output["data"].as_object() {
        Some(data) => data.keys().cloned().collect(),
        None => vec![
            output["output_type"]
                .as_str()
                .unwrap_or("output")
                .to_string(),
        ],
    }
}

fn render(cells: &[Map<String, Value>]) -> String {
    let mut out = String::new();
    for (i, cell) in cells.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format!("# %% [{}] cell:{}\n", cell_kind(cell), i + 1));
        out.push_str(&source_text(cell));
        out.push('\n');
        if let Some(summary) = output_summary(cell) {
            out.push_str(&summary);
            out.push('\n');
        }
    }
    out
}

/// One cell of a rendering: its marker and the lines up to the next one.
struct Block {
    kind: String,
    /// What follows `cell:` (empty when the marker has no number).
    tag: String,
    /// 1-based line of the marker.
    line: usize,
    body: String,
}

/// `(kind, tag)` when `line` is a marker: `# %% [kind]` or `# %% [kind] cell:tag`.
fn marker(line: &str) -> Option<(&str, &str)> {
    let (kind, tail) = line.strip_prefix("# %% [")?.split_once(']')?;
    match tail.strip_prefix(" cell:") {
        Some(tag) => Some((kind, tag)),
        None if tail.is_empty() => Some((kind, "")),
        None => None,
    }
}

fn split_blocks(text: &str) -> Result<Vec<Block>, String> {
    let mut blocks: Vec<Block> = Vec::new();
    for (n, line) in text.split_inclusive('\n').enumerate() {
        match (marker(line.trim_end()), blocks.last_mut()) {
            (Some((kind, tag)), _) => blocks.push(Block {
                kind: kind.to_string(),
                tag: tag.to_string(),
                line: n + 1,
                body: String::new(),
            }),
            (None, Some(block)) => block.body.push_str(line),
            (None, None) if line.trim().is_empty() => {}
            (None, None) => {
                return Err(format!("line {}: text before the first cell marker", n + 1));
            }
        }
    }
    Ok(blocks)
}

/// A block's cell source: the body without the blank line that separates it
/// from the next block, the output summary, and the newline after the source.
fn source_of(block: &Block, last: bool, summary: Option<&str>) -> String {
    let mut body = block.body.as_str();
    if !last {
        body = body.strip_suffix('\n').unwrap_or(body);
    }
    if let Some(rest) = summary.and_then(|s| body.strip_suffix(&format!("{s}\n")))
        && rest.ends_with('\n')
    {
        body = rest;
    }
    body.strip_suffix('\n').unwrap_or(body).to_string()
}

/// `cell` with a new type and source. An unchanged cell is kept as it was;
/// a changed code cell loses its outputs and execution count.
fn updated_cell(cell: &Map<String, Value>, kind: &str, source: &str) -> Value {
    let mut cell = cell.clone();
    if cell_kind(&cell) == kind && source_text(&cell) == source {
        return Value::Object(cell);
    }
    let shaped = source_value(source, cell.get("source"));
    cell.insert("source".into(), shaped);
    cell.insert("cell_type".into(), Value::from(kind));
    if kind == "code" {
        cell.insert("execution_count".into(), Value::Null);
        cell.insert("outputs".into(), json!([]));
    } else {
        cell.remove("execution_count");
        cell.remove("outputs");
    }
    Value::Object(cell)
}

fn new_cell(kind: &str, source: &str, uses_ids: bool, ids: &mut HashSet<String>) -> Value {
    let mut cell = Map::new();
    cell.insert("cell_type".into(), Value::from(kind));
    if kind == "code" {
        cell.insert("execution_count".into(), Value::Null);
        cell.insert("outputs".into(), json!([]));
    }
    if uses_ids {
        cell.insert("id".into(), Value::from(fresh_id(source, ids)));
    }
    cell.insert("metadata".into(), json!({}));
    cell.insert("source".into(), source_value(source, None));
    Value::Object(cell)
}

/// An 8-hex-digit cell id not yet in `ids`.
fn fresh_id(seed: &str, ids: &mut HashSet<String>) -> String {
    let mut salt = 0u64;
    loop {
        let id = format!("{:08x}", hash_u64(&format!("{seed}\0{salt}")) as u32);
        if ids.insert(id.clone()) {
            return id;
        }
        salt += 1;
    }
}

/// `value` as JSON in the style of `like`: its indent (compact when it has
/// no line breaks), line endings and trailing newline.
fn serialize(value: &Value, like: &str) -> String {
    let mut out = String::new();
    match like.trim_start().split_once('\n') {
        Some((_, rest)) => {
            let indent: String = rest
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            pretty(&mut out, value, &indent, 0);
        }
        None => out.push_str(&value.to_string()),
    }
    if like.ends_with('\n') {
        out.push('\n');
    }
    if tolerant::is_crlf(like) {
        out = tolerant::to_crlf(&out);
    }
    out
}

/// Indented JSON as Python's `json.dumps(indent=…)` writes it (Jupyter's
/// writer): `,` between items, `": "` after keys, `[]` / `{}` when empty.
fn pretty(out: &mut String, value: &Value, indent: &str, depth: usize) {
    match value {
        Value::Array(items) if !items.is_empty() => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                item_start(out, i, indent, depth + 1);
                pretty(out, item, indent, depth + 1);
            }
            newline(out, indent, depth);
            out.push(']');
        }
        Value::Object(map) if !map.is_empty() => {
            out.push('{');
            for (i, (key, item)) in map.iter().enumerate() {
                item_start(out, i, indent, depth + 1);
                out.push_str(&Value::from(key.as_str()).to_string());
                out.push_str(": ");
                pretty(out, item, indent, depth + 1);
            }
            newline(out, indent, depth);
            out.push('}');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

fn item_start(out: &mut String, i: usize, indent: &str, depth: usize) {
    if i > 0 {
        out.push(',');
    }
    newline(out, indent, depth);
}

fn newline(out: &mut String, indent: &str, depth: usize) {
    out.push('\n');
    out.push_str(&indent.repeat(depth));
}

#[cfg(test)]
#[path = "notebook_tests.rs"]
mod tests;
