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
    if let Some(root) = git_root(project_path) {
        let mut cur = project_path.to_path_buf();
        while cur != root {
            match cur.parent() {
                Some(parent) => {
                    cur = parent.to_path_buf();
                    dirs.push(cur.clone());
                }
                None => break,
            }
        }
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
        dir.join(".whycodes").join("AGENTS.md"),
        dir.join(".whycodes").join("RULES.md"),
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
mod tests {
    use super::*;

    #[test]
    fn empty_project_discovers_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(discover(dir.path()).is_empty());
        let prompt = append_project_instructions("base", dir.path());
        assert_eq!(prompt, "base");
        assert!(!prompt.contains("Project Instructions"));
    }

    #[test]
    fn agents_md_keeps_existing_heading() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "  \nProject rules here\n  ").unwrap();
        let with = append_project_instructions("base", dir.path());
        assert!(with.contains("Project Instructions (AGENTS.md)"), "{with}");
        assert!(with.contains("Project rules here"), "{with}");
    }

    #[test]
    fn lowercase_agents_md_when_canonical_absent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("agents.md"), "lowercase rules").unwrap();
        let with = append_project_instructions("base", dir.path());
        assert!(with.contains("lowercase rules"), "{with}");
    }

    #[test]
    fn whycodes_nested_agents_md() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".whycodes")).unwrap();
        std::fs::write(dir.path().join(".whycodes/AGENTS.md"), "nested rules").unwrap();
        let with = append_project_instructions("base", dir.path());
        assert!(with.contains("nested rules"), "{with}");
        assert!(with.contains(".whycodes/AGENTS.md"), "{with}");
    }

    #[test]
    fn sibling_claude_and_copilot_files_are_concatenated() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "native").unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "claude rules").unwrap();
        std::fs::create_dir(dir.path().join(".github")).unwrap();
        std::fs::write(
            dir.path().join(".github/copilot-instructions.md"),
            "copilot rules",
        )
        .unwrap();
        let files = discover(dir.path());
        let joined: String = files.iter().map(|f| f.content.as_str()).collect();
        assert!(joined.contains("native"));
        assert!(joined.contains("claude rules"));
        assert!(joined.contains("copilot rules"));
        let rendered = render(&files);
        assert!(rendered.contains("Additional instructions (CLAUDE.md)"));
    }

    #[test]
    fn duplicate_content_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "same body").unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "same body").unwrap();
        let files = discover(dir.path());
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].label, "AGENTS.md");
    }

    #[test]
    fn cursor_mdc_and_clinerules_dir_are_picked_up() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".cursor/rules")).unwrap();
        std::fs::write(
            dir.path().join(".cursor/rules/rust.mdc"),
            "always use cargo fmt",
        )
        .unwrap();
        std::fs::create_dir(dir.path().join(".clinerules")).unwrap();
        std::fs::write(dir.path().join(".clinerules/style.md"), "no unwrap").unwrap();
        std::fs::write(dir.path().join(".cursorrules"), "cursor root").unwrap();
        let files = discover(dir.path());
        let labels: Vec<&str> = files.iter().map(|f| f.label.as_str()).collect();
        assert!(labels.iter().any(|l| l.contains("rust.mdc")), "{labels:?}");
        assert!(labels.iter().any(|l| l.contains("style.md")), "{labels:?}");
        assert!(labels.contains(&".cursorrules"), "{labels:?}");
    }

    #[test]
    fn git_root_walk_collects_ancestor_agents() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "root rules").unwrap();
        let pkg = dir.path().join("pkg");
        std::fs::create_dir(&pkg).unwrap();
        std::fs::write(pkg.join("CLAUDE.md"), "pkg rules").unwrap();
        let files = discover(&pkg);
        let joined: String = files.iter().map(|f| f.content.as_str()).collect();
        assert!(joined.contains("pkg rules"), "{joined}");
        assert!(joined.contains("root rules"), "{joined}");
    }

    #[test]
    fn empty_files_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "   \n").unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "keep").unwrap();
        let files = discover(dir.path());
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].content, "keep");
    }

    #[test]
    fn label_falls_back_to_file_name_outside_project() {
        assert_eq!(
            label_for(Path::new("/tmp/CLAUDE.md"), Path::new("/proj")),
            "CLAUDE.md"
        );
    }
}
