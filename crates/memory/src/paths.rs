//! On-disk layout for human-editable auto memory (Claude Code spirit).

use std::path::{Path, PathBuf};

use crate::project_key::project_key;

/// `{data_dir}/memory/<project_key>/`
pub fn memory_dir(data_dir: &Path, project_path: &Path) -> PathBuf {
    data_dir.join("memory").join(project_key(project_path))
}

/// `{memory_dir}/MEMORY.md`
pub fn memory_md(data_dir: &Path, project_path: &Path) -> PathBuf {
    memory_dir(data_dir, project_path).join("MEMORY.md")
}

/// Ensure the memory directory exists.
pub fn ensure_memory_dir(data_dir: &Path, project_path: &Path) -> std::io::Result<PathBuf> {
    let dir = memory_dir(data_dir, project_path);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}
