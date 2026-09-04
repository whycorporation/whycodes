//! Cross-session semantic / auto memory for whycodes.
//!
//! - **Auto memory**: human-editable `MEMORY.md` (Claude Code parity)
//! - **Semantic facts**: SQLite + embeddings, auto-recall (jcode / Grok style)
//! - **Auto-retain**: post-turn heuristic extraction (Hindsight spirit)
//! - **Code RAG**: lightweight chunk index over the repo
//! - **Scopes**: user (data_dir) or project (`.whycodes/memory`, git-shareable)
//! - **Agent banks**: per-subagent memory isolation
//! - **ONNX MiniLM**: optional (`--features onnx`)

pub mod code_index;
pub mod embed;
pub mod error;
pub mod markdown;
pub mod onnx;
pub mod paths;
pub mod project_key;
pub mod retain;
pub mod service;
pub mod settings;
pub mod sync;

pub use embed::{DEFAULT_DIM, cosine, decode_blob, embed, encode_blob};
pub use error::{MemoryError, Result};
pub use project_key::{project_key, project_root};
pub use retain::{extract_heuristic, llm_retain_prompt, parse_llm_facts};
pub use service::{
    CodeHit, MemoryService, RecallHit, SessionHit, apply_memory_prompt, maybe_auto_index,
    maybe_auto_retain, settings_from_flags,
};
pub use settings::{EmbedBackend, MemoryScope, MemorySettings};

#[cfg(test)]
pub(crate) static TEST_PATH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_helpers_are_callable() {
        let v = embed("cargo test memory", DEFAULT_DIM);
        assert!((cosine(&v, &v) - 1.0).abs() < 1e-5);
        assert_eq!(decode_blob(&encode_blob(&[1.0, 2.0])).len(), 2);
        assert!(!project_key(std::path::Path::new("/tmp")).is_empty());
        assert!(llm_retain_prompt("u", "a").contains("USER:\nu"));
        assert!(settings_from_flags(true).enabled);
        assert!(!settings_from_flags(false).enabled);
        assert_eq!(EmbedBackend::Hash.as_str(), "hash");
        assert_eq!(MemoryScope::User.as_str(), "user");
    }
}
