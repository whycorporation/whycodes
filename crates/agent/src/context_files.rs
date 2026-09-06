//! Discover project instruction files already written for other coding agents.
//!
//! WhyCodes' native file is `AGENTS.md`. Sibling conventions (Claude, Gemini,
//! Copilot, Cursor, Cline, Windsurf) are loaded from the same project so a
//! checkout does not need a migration step.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Soft cap so a monorepo of instruction files cannot blow the system prompt.
const MAX_CONTEXT_BYTES: usize = 80_000;
/// Hard cap on how many files we concatenate.
const MAX_CONTEXT_FILES: usize = 24;

/// Append discovered project instruction files to `system_prompt`.
///
/// Returns `system_prompt` unchanged when nothing is found. Does not attach
/// runtime context — callers should pass the result through
/// [`crate::agent::Agent::with_runtime_context`].
pub fn append_project_instructions(system_prompt: &str, project_path: &Path) -> String {
    let files = discover(project_path);
    if files.is_empty() {
        return system_prompt.to_string();
    }
    let mut out = String::from(system_prompt);
    out.push_str("\n\n");
    out.push_str(&render(&files));
    out
}

/// One instruction file, labelled for the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextFile {
    pub label: String,
    pub content: String,
}

pub fn discover(project_path: &Path) -> Vec<ContextFile> {
    let mut out = Vec::new();
    let mut seen_content = BTreeSet::new();
    let mut seen_paths = BTreeSet::new();
    let mut bytes = 0usize;

    for dir in scan_dirs(project_path) {
        for path in candidates_in(&dir) {
            if !seen_paths.insert(path.clone()) {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !seen_content.insert(trimmed.to_string()) {
                continue;
            }
            let add = trimmed.len();
            if bytes + add > MAX_CONTEXT_BYTES && !out.is_empty() {
                break;
            }
            let label = label_for(&path, project_path);
            bytes = bytes.saturating_add(add);
            out.push(ContextFile {
                label,
                content: trimmed.to_string(),
            });
            if out.len() >= MAX_CONTEXT_FILES {
                return out;
            }
        }
        if out.len() >= MAX_CONTEXT_FILES || bytes >= MAX_CONTEXT_BYTES {
            break;
        }
    }
    out
}

fn render(files: &[ContextFile]) -> String {
    let mut out = String::new();
    for (i, file) in files.iter().enumerate() {
        if i == 0 {
            out.push_str("# Project Instructions (");
            out.push_str(&file.label);
            out.push_str(")\n\n");
        } else {
            out.push_str("\n# Additional instructions (");
            out.push_str(&file.label);
            out.push_str(")\n\n");
        }
        out.push_str(&file.content);
        out.push('\n');
    }
    out
}

fn scan_dirs(project_path: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![project_path.to_path_buf()];
    let Some(root) = git_root(project_path) else {
        return dirs;
    };
    let mut cur = project_path.to_path_buf();
    while cur != root {
        // `git_root` only returns an ancestor; jumping to `root` ends the walk
        // if a parent is ever missing (filesystem root).
        cur = cur
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| root.clone());
        dirs.push(cur.clone());
    }
    dirs
}

fn git_root(start: &Path) -> Option<PathBuf> {
    let mut cur = start;
    loop {
        if cur.join(".git").exists() {
            return Some(cur.to_path_buf());
        }
        cur = cur.parent()?;
    }
}

fn candidates_in(dir: &Path) -> Vec<PathBuf> {
    let mut paths = vec![
        dir.join("AGENTS.md"),
        dir.join("agents.md"),
        dir.join("CLAUDE.md"),
        dir.join("GEMINI.md"),
        dir.join("RULES.md"),
        dir.join(".cursorrules"),
        dir.join(".windsurfrules"),
        dir.join(".clinerules"),
        whycodes_core::project_dir(dir).join("AGENTS.md"),
        whycodes_core::project_dir(dir).join("RULES.md"),
        dir.join(".claude").join("CLAUDE.md"),
        dir.join(".gemini").join("GEMINI.md"),
        dir.join(".github").join("copilot-instructions.md"),
    ];
    push_glob_files(dir.join(".cursor").join("rules"), "mdc", &mut paths);
    push_glob_files(dir.join(".clinerules"), "md", &mut paths);
    push_glob_files(dir.join(".github").join("instructions"), "md", &mut paths);
    paths
}

fn push_glob_files(dir: PathBuf, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut extra: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case(ext))
        })
        .collect();
    extra.sort();
    out.extend(extra);
}

fn label_for(path: &Path, project_path: &Path) -> String {
    path.strip_prefix(project_path)
        .map(|rel| rel.display().to_string())
        .unwrap_or_else(|_| {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string())
        })
        .replace('\\', "/")
}

#[cfg(test)]
#[path = "context_files_tests.rs"]
mod tests;
