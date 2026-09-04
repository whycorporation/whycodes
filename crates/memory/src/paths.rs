//! On-disk layout for human-editable auto memory (Claude Code spirit).

use std::path::{Path, PathBuf};

use crate::project_key::project_key;
use crate::settings::MemoryScope;

/// Directory holding MEMORY.md for this bank.
///
/// - **User scope:** `{data_dir}/memory/<project_key>[/agents/<bank>]/`
/// - **Project scope:** `{project}/.whycodes/memory[/agents/<bank>]` (git-shareable)
pub fn memory_dir(
    data_dir: &Path,
    project_path: &Path,
    scope: MemoryScope,
    agent_bank: Option<&str>,
) -> PathBuf {
    let base = match scope {
        MemoryScope::User => data_dir.join("memory").join(project_key(project_path)),
        MemoryScope::Project => whycodes_core::project_dir(project_path).join("memory"),
    };
    match agent_bank {
        Some(a) if !a.is_empty() => base.join("agents").join(sanitize_component(a)),
        _ => base,
    }
}

pub fn memory_md(
    data_dir: &Path,
    project_path: &Path,
    scope: MemoryScope,
    agent_bank: Option<&str>,
) -> PathBuf {
    memory_dir(data_dir, project_path, scope, agent_bank).join("MEMORY.md")
}

pub fn ensure_memory_dir(
    data_dir: &Path,
    project_path: &Path,
    scope: MemoryScope,
    agent_bank: Option<&str>,
) -> std::io::Result<PathBuf> {
    let dir = memory_dir(data_dir, project_path, scope, agent_bank);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn sanitize_component(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::MemoryScope;
    use std::path::Path;

    #[test]
    fn paths_module_loads() {
        assert!(!module_path!().is_empty());
    }

    #[test]
    fn user_and_project_dirs_sanitize_agent_bank() {
        let data = Path::new("/tmp/data");
        let project = Path::new("/tmp/proj");
        let user = memory_dir(data, project, MemoryScope::User, None);
        assert!(user.iter().any(|c| c == "memory"));
        let agent = memory_dir(data, project, MemoryScope::User, Some("bot/name"));
        assert!(agent.ends_with("bot_name"));
        let empty_bank = memory_dir(data, project, MemoryScope::Project, Some(""));
        assert!(empty_bank.ends_with("memory"));
        assert_eq!(sanitize_component("a b/c"), "a_b_c");
        let md = memory_md(data, project, MemoryScope::Project, Some("explore"));
        assert!(md.ends_with("MEMORY.md"));
        let dir = tempfile::tempdir().unwrap();
        let created =
            ensure_memory_dir(dir.path(), dir.path(), MemoryScope::User, Some("bank-1")).unwrap();
        assert!(created.exists());
    }
}
