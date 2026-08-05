//! Runtime settings for memory inject / recall (decoupled from config crate).

/// Inject and store settings used by [`crate::MemoryService`].
#[derive(Debug, Clone)]
pub struct MemorySettings {
    pub enabled: bool,
    pub auto_inject: bool,
    pub max_index_lines: usize,
    pub max_index_bytes: usize,
    pub recall_top_k: usize,
    pub recall_min_score: f32,
    pub recall_token_budget: usize,
    pub embed_dim: usize,
}

impl Default for MemorySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_inject: true,
            max_index_lines: 200,
            max_index_bytes: 25_600,
            recall_top_k: 5,
            recall_min_score: 0.28,
            recall_token_budget: 800,
            embed_dim: crate::embed::DEFAULT_DIM,
        }
    }
}

impl MemorySettings {
    /// Disabled settings (no inject, writes should refuse at call sites).
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }

    /// Approximate char budget from token budget (same heuristic as sessions).
    pub fn recall_char_budget(&self) -> usize {
        self.recall_token_budget.saturating_mul(4)
    }
}
