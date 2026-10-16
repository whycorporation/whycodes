//! On-disk artifacts for old tool outputs. The session keeps the head of a
//! spilled body plus a pointer (`Session::spill_old_tool_results`); this is
//! the store that writes the full body under
//! `.whycodes/scratch/artifacts/<tool_call_id>.txt` and names it for `read`.

use std::path::{Path, PathBuf};

/// Directory under the project scratch dir.
pub const ARTIFACT_DIR: &str = "artifacts";

/// A store for [`whycodes_session::session::Session::spill_old_tool_results`]
/// bound to one project.
pub(crate) fn artifact_store(project: PathBuf) -> impl FnMut(&str, &str) -> Option<String> {
    move |id, body| store_artifact(&project, id, body)
}

/// Write `body` once and return the project-relative pointer the model can
/// pass to `read`. An unwritable scratch directory keeps the body in the
/// transcript (the caller leaves it alone).
pub(crate) fn store_artifact(project: &Path, id: &str, body: &str) -> Option<String> {
    let name = format!("{}.txt", safe_file_stem(id));
    let dir = whycodes_core::project_scratch_dir(project).join(ARTIFACT_DIR);
    if let Err(err) = std::fs::create_dir_all(&dir) {
        tracing::debug!(error = %err, "tool artifact dir not created; body stays in the transcript");
        return None;
    }
    let path = dir.join(&name);
    if !path.is_file()
        && let Err(err) = std::fs::write(&path, body)
    {
        tracing::debug!(error = %err, "tool artifact not written; body stays in the transcript");
        return None;
    }
    // Project-relative with forward slashes on every OS: what `read` takes.
    let rel = whycodes_core::project_scratch_dir(Path::new(""))
        .join(ARTIFACT_DIR)
        .join(&name);
    Some(rel.to_string_lossy().replace('\\', "/"))
}

/// Tool call ids are provider-chosen; keep them file-system safe.
fn safe_file_stem(id: &str) -> String {
    let stem: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    if stem.is_empty() { "tool".into() } else { stem }
}

#[cfg(test)]
#[path = "spill_tests.rs"]
mod tests;
