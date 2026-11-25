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
///
/// The name carries a hash of the body. Ids alone repeat: Gemini numbers
/// its calls from `gcall_1` in every stream, and other sessions in the
/// project share the directory. Keyed by id only, a later result pointed at
/// an earlier call's file. The same body still maps to the same file.
pub(crate) fn store_artifact(project: &Path, id: &str, body: &str) -> Option<String> {
    let name = format!("{}-{:016x}.txt", safe_file_stem(id), fnv1a(body));
    let dir = whycodes_core::project_scratch_dir(project).join(ARTIFACT_DIR);
    if let Err(err) =
        whycodes_core::create_scratch_dir(project).and_then(|_| std::fs::create_dir_all(&dir))
    {
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

/// FNV-1a 64: stable across runs and builds (unlike `DefaultHasher`), so a
/// re-spilled body finds the file it was written to.
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
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
