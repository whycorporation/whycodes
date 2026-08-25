//! Runtime settings for memory inject / recall / retain / index.

/// Where durable memory lives (Claude-style scopes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MemoryScope {
    /// Machine-local under data_dir (default; Claude auto-memory parity).
    #[default]
    User,
    /// Project-local under `.whycodes/memory/` (git-shareable cross-machine).
    Project,
}

impl MemoryScope {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "project" | "repo" | "local" => Self::Project,
            _ => Self::User,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

/// Embedding backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmbedBackend {
    #[default]
    Hash,
    /// MiniLM via ONNX (optional cargo feature `onnx`; falls back to hash if unavailable).
    Onnx,
}

impl EmbedBackend {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "onnx" | "minilm" | "neural" => Self::Onnx,
            _ => Self::Hash,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hash => "hash",
            Self::Onnx => "onnx",
        }
    }
}

/// Inject and store settings used by [`crate::MemoryService`].
#[derive(Debug, Clone)]
pub struct MemorySettings {
    pub enabled: bool,
    pub auto_inject: bool,
    /// Post-turn auto-retain (Hindsight-style). Default on with heuristic extract.
    pub auto_retain: bool,
    /// Call a small LLM for fact extraction when heuristic is empty (or always).
    pub retain_llm: bool,
    /// Run LLM retain even when heuristic already found facts.
    pub retain_llm_always: bool,
    /// Only run retain every N completed user turns (1 = every turn).
    pub retain_every_n: usize,
    /// Max new facts retained per turn.
    pub retain_max_facts: usize,
    pub max_index_lines: usize,
    pub max_index_bytes: usize,
    pub recall_top_k: usize,
    pub recall_min_score: f32,
    pub recall_token_budget: usize,
    pub embed_dim: usize,
    pub scope: MemoryScope,
    pub embed_backend: EmbedBackend,
    /// Optional agent bank suffix (subagent-scoped memory). Empty = main bank.
    pub agent_bank: Option<String>,
    /// Inject top-k code RAG hits when query present.
    pub code_inject: bool,
    pub code_top_k: usize,
    pub code_min_score: f32,
    /// Ensure code index on session start if empty.
    pub auto_index: bool,
    pub auto_index_max_files: usize,
    pub auto_index_max_chunks: usize,
    /// Subagents get isolated banks when true.
    pub subagent_banks: bool,
    /// Inject past-session turn hits.
    pub session_inject: bool,
    pub session_top_k: usize,
    pub session_min_score: f32,
    /// After retain, drop least-used facts if the bank is over this size.
    pub consolidate: bool,
    pub consolidate_max: usize,
}

impl Default for MemorySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_inject: true,
            auto_retain: true,
            retain_llm: true,
            retain_llm_always: false,
            retain_every_n: 1,
            retain_max_facts: 3,
            max_index_lines: 200,
            max_index_bytes: 25_600,
            recall_top_k: 5,
            recall_min_score: 0.28,
            recall_token_budget: 800,
            embed_dim: crate::embed::DEFAULT_DIM,
            scope: MemoryScope::User,
            embed_backend: EmbedBackend::Hash,
            agent_bank: None,
            code_inject: true,
            code_top_k: 4,
            code_min_score: 0.22,
            auto_index: true,
            auto_index_max_files: 1500,
            auto_index_max_chunks: 4000,
            subagent_banks: true,
            session_inject: true,
            session_top_k: 3,
            session_min_score: 0.22,
            consolidate: true,
            consolidate_max: 80,
        }
    }
}

impl MemorySettings {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }

    pub fn recall_char_budget(&self) -> usize {
        self.recall_token_budget.saturating_mul(4)
    }

    /// Bank key for SQLite rows: `project` or `project::agent`.
    pub fn bank_key(&self, project_key: &str) -> String {
        match &self.agent_bank {
            Some(a) if !a.is_empty() => format!("{project_key}::{}", sanitize_bank(a)),
            _ => project_key.to_string(),
        }
    }
}

fn sanitize_bank(s: &str) -> String {
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
