//! Cross-session semantic / auto memory for whycode.
//!
//! - **Auto memory**: human-editable `MEMORY.md` index (Claude Code parity)
//! - **Semantic facts**: SQLite + hashing embeddings, auto-recall by cosine
//!   similarity (jcode / Grok Hindsight style without ONNX)

pub mod embed;
pub mod markdown;
pub mod paths;
pub mod project_key;
pub mod service;
pub mod settings;

pub use embed::{DEFAULT_DIM, cosine, decode_blob, embed, encode_blob};
pub use project_key::{project_key, project_root};
pub use service::{MemoryService, RecallHit, apply_memory_prompt, settings_from_flags};
pub use settings::MemorySettings;
