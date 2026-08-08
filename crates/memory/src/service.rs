//! High-level memory API: dual-write SQLite + MEMORY.md, inject, retain.

use std::path::{Path, PathBuf};

use whycode_storage::db::Database;
use whycode_storage::models::MemoryRow;

// CodeHit re-exports the storage row type for callers.
pub use whycode_storage::models::CodeChunkRow;

use crate::embed::{cosine, decode_blob, embed, encode_blob};
use crate::markdown;
use crate::paths::{ensure_memory_dir, memory_md};
use crate::project_key::project_key;
use crate::retain;
use crate::settings::{EmbedBackend, MemorySettings};

/// A scored recall hit (facts).
#[derive(Debug, Clone)]
pub struct RecallHit {
    pub entry: MemoryRow,
    pub score: f32,
}

/// A scored code RAG hit.
#[derive(Debug, Clone)]
pub struct CodeHit {
    pub entry: CodeChunkRow,
    pub score: f32,
}

/// Project-scoped memory service.
pub struct MemoryService {
    pub project_key: String,
    pub bank_key: String,
    pub project_path: PathBuf,
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub settings: MemorySettings,
}

impl MemoryService {
    pub fn open(
        project_path: impl Into<PathBuf>,
        data_dir: impl Into<PathBuf>,
        settings: MemorySettings,
    ) -> anyhow::Result<Self> {
        let project_path = project_path.into();
        let data_dir = data_dir.into();
        let key = project_key(&project_path);
        let bank_key = settings.bank_key(&key);
        let db_path = data_dir.join("whycode.db");
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        ensure_memory_dir(
            &data_dir,
            &project_path,
            settings.scope,
            settings.agent_bank.as_deref(),
        )?;
        Ok(Self {
            project_key: key,
            bank_key,
            project_path,
            data_dir,
            db_path,
            settings,
        })
    }

    pub fn open_db(&self) -> anyhow::Result<Database> {
        Database::open(self.db_path.to_str().unwrap_or("whycode.db"))
    }

    pub fn memory_md_path(&self) -> PathBuf {
        memory_md(
            &self.data_dir,
            &self.project_path,
            self.settings.scope,
            self.settings.agent_bank.as_deref(),
        )
    }

    /// Embed text with configured backend (ONNX falls back to hash).
    pub fn embed_text(&self, text: &str) -> Vec<f32> {
        if self.settings.embed_backend == EmbedBackend::Onnx {
            if let Some(v) = crate::onnx::try_embed(text, &self.data_dir) {
                return v;
            }
            tracing::debug!("onnx unavailable; using hash embedder");
        }
        embed(text, self.settings.embed_dim)
    }

    pub fn remember(&self, text: &str, source_session: Option<&str>) -> anyhow::Result<String> {
        if !self.settings.enabled {
            anyhow::bail!("memory is disabled");
        }
        let text = text.trim();
        if text.is_empty() {
            anyhow::bail!("memory text is empty");
        }
        if self.is_duplicate(text)? {
            anyhow::bail!("duplicate memory (already stored)");
        }
        let id = uuid::Uuid::new_v4().to_string();
        let short_id = &id[..8.min(id.len())];
        let vec = self.embed_text(text);
        let blob = encode_blob(&vec);
        let db = self.open_db()?;
        db.insert_memory(&id, &self.bank_key, text, &blob, source_session)?;
        markdown::append_entry(&self.memory_md_path(), short_id, text)?;
        Ok(id)
    }

    /// True if an existing fact is nearly identical (cosine ≥ 0.92).
    pub fn is_duplicate(&self, text: &str) -> anyhow::Result<bool> {
        let hits = self.search(text, 1, 0.92)?;
        Ok(!hits.is_empty())
    }

    pub fn list(&self, limit: usize) -> anyhow::Result<Vec<MemoryRow>> {
        let db = self.open_db()?;
        db.list_memories(&self.bank_key, limit)
    }

    pub fn delete(&self, id_or_prefix: &str) -> anyhow::Result<bool> {
        if !self.settings.enabled {
            anyhow::bail!("memory is disabled");
        }
        let db = self.open_db()?;
        let id = resolve_id(&db, &self.bank_key, id_or_prefix)?;
        let Some(id) = id else {
            return Ok(false);
        };
        let removed = db.delete_memory(&id)?;
        if removed {
            let short = &id[..8.min(id.len())];
            let _ = markdown::remove_entry(&self.memory_md_path(), short);
            let _ = markdown::remove_entry(&self.memory_md_path(), &id);
        }
        Ok(removed)
    }

    pub fn clear(&self) -> anyhow::Result<usize> {
        if !self.settings.enabled {
            anyhow::bail!("memory is disabled");
        }
        let db = self.open_db()?;
        let n = db.clear_memories(&self.bank_key)?;
        markdown::clear_file(&self.memory_md_path())?;
        Ok(n)
    }

    pub fn search(
        &self,
        query: &str,
        top_k: usize,
        min_score: f32,
    ) -> anyhow::Result<Vec<RecallHit>> {
        let db = self.open_db()?;
        let rows = db.list_memories(&self.bank_key, 10_000)?;
        let q = self.embed_text(query);
        let mut hits: Vec<RecallHit> = rows
            .into_iter()
            .filter_map(|entry| {
                let v = decode_blob(&entry.embedding);
                if v.is_empty() || v.len() != q.len() {
                    // Dim mismatch (hash vs onnx) — skip
                    return None;
                }
                let score = cosine(&q, &v);
                if score >= min_score {
                    Some(RecallHit { entry, score })
                } else {
                    None
                }
            })
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(top_k.max(1));
        Ok(hits)
    }

    /// Post-turn auto-retain (heuristic). Returns saved fact texts.
    pub fn auto_retain(
        &self,
        user_text: &str,
        assistant_text: Option<&str>,
        source_session: Option<&str>,
        turn_index: usize,
    ) -> anyhow::Result<Vec<String>> {
        if !self.settings.enabled || !self.settings.auto_retain {
            return Ok(Vec::new());
        }
        let every = self.settings.retain_every_n.max(1);
        if turn_index > 0 && !turn_index.is_multiple_of(every) {
            return Ok(Vec::new());
        }
        let mut candidates = retain::extract_heuristic(user_text, assistant_text);
        candidates.truncate(self.settings.retain_max_facts);
        self.save_facts(candidates, source_session)
    }

    /// Whether the LLM retain pass should run for this turn.
    pub fn should_run_llm_retain(&self, heuristic_saved: usize, turn_index: usize) -> bool {
        if !self.settings.enabled || !self.settings.auto_retain || !self.settings.retain_llm {
            return false;
        }
        let every = self.settings.retain_every_n.max(1);
        if turn_index > 0 && !turn_index.is_multiple_of(every) {
            return false;
        }
        self.settings.retain_llm_always || heuristic_saved == 0
    }

    fn save_facts(
        &self,
        candidates: Vec<String>,
        source_session: Option<&str>,
    ) -> anyhow::Result<Vec<String>> {
        let mut saved = Vec::new();
        for fact in candidates {
            match self.remember(&fact, source_session) {
                Ok(_) => saved.push(fact),
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("duplicate") {
                        continue;
                    }
                    tracing::debug!("auto_retain skip: {msg}");
                }
            }
        }
        Ok(saved)
    }

    /// Ensure a code index exists for this bank (no-op if already non-empty).
    /// Returns `Some(n)` when a new index was built, `None` when skipped.
    pub fn ensure_code_index(&self) -> anyhow::Result<Option<usize>> {
        if !self.settings.enabled || !self.settings.auto_index {
            return Ok(None);
        }
        let db = self.open_db()?;
        let existing = db.list_code_chunks(&self.bank_key, 1)?;
        if !existing.is_empty() {
            return Ok(None);
        }
        let n = self.index_codebase(
            self.settings.auto_index_max_files,
            self.settings.auto_index_max_chunks,
        )?;
        Ok(Some(n))
    }

    /// Retain from pre-parsed LLM fact lines.
    pub fn retain_llm_facts(
        &self,
        raw_llm: &str,
        source_session: Option<&str>,
    ) -> anyhow::Result<Vec<String>> {
        if !self.settings.enabled || !self.settings.auto_retain {
            return Ok(Vec::new());
        }
        let candidates: Vec<String> = retain::parse_llm_facts(raw_llm)
            .into_iter()
            .take(self.settings.retain_max_facts)
            .collect();
        self.save_facts(candidates, source_session)
    }

    pub fn build_inject_block(&self, query: Option<&str>) -> anyhow::Result<String> {
        if !self.settings.enabled {
            return Ok(String::new());
        }

        let mut parts: Vec<String> = Vec::new();
        let mut char_budget = self.settings.recall_char_budget()
            + self
                .settings
                .max_index_bytes
                .min(self.settings.recall_char_budget() * 2);

        let index = markdown::load_capped(
            &self.memory_md_path(),
            self.settings.max_index_lines,
            self.settings.max_index_bytes,
        );
        if !index.is_empty() {
            let scope = self.settings.scope.as_str();
            let bank = self.settings.agent_bank.as_deref().unwrap_or("main");
            let block = format!(
                "# Auto Memory\n\n\
                 Notes saved for this project (scope={scope}, bank={bank}). Prefer repo truth if stale.\n\n\
                 {index}"
            );
            char_budget = char_budget.saturating_sub(block.len());
            parts.push(block);
        }

        if self.settings.auto_inject
            && let Some(q) = query.map(str::trim).filter(|s| !s.is_empty())
        {
            let hits = self.search(
                q,
                self.settings.recall_top_k,
                self.settings.recall_min_score,
            )?;
            if !hits.is_empty() {
                let mut lines = Vec::new();
                let mut used = 0usize;
                let header = "# Recalled Memories (from prior sessions; verify if stale)\n";
                used += header.len();
                let mut ids = Vec::new();
                for hit in &hits {
                    let line = format!(
                        "- [{:.2}] {} (id:{})\n",
                        hit.score,
                        hit.entry.text.trim(),
                        &hit.entry.id[..8.min(hit.entry.id.len())]
                    );
                    if used + line.len() > char_budget && !lines.is_empty() {
                        break;
                    }
                    used += line.len();
                    lines.push(line);
                    ids.push(hit.entry.id.clone());
                }
                if !lines.is_empty() {
                    parts.push(format!("{header}{}", lines.join("")));
                    if let Ok(db) = self.open_db() {
                        for id in ids {
                            let _ = db.touch_memory_recall(&id);
                        }
                    }
                }
            }

            // Code RAG
            if self.settings.code_inject
                && let Ok(code_hits) =
                    self.search_code(q, self.settings.code_top_k, self.settings.code_min_score)
                && !code_hits.is_empty()
            {
                let mut lines = Vec::new();
                let mut used = 0usize;
                let header = "# Code context (indexed; verify in repo)\n";
                used += header.len();
                for hit in &code_hits {
                    let snippet = hit
                        .entry
                        .text
                        .lines()
                        .take(8)
                        .collect::<Vec<_>>()
                        .join("\n");
                    let line = format!(
                        "### {} ({}-{}) [{:.2}]\n```\n{}\n```\n",
                        hit.entry.path,
                        hit.entry.start_line,
                        hit.entry.end_line,
                        hit.score,
                        snippet
                    );
                    if used + line.len() > char_budget && !lines.is_empty() {
                        break;
                    }
                    used += line.len();
                    lines.push(line);
                }
                if !lines.is_empty() {
                    parts.push(format!("{header}{}", lines.join("\n")));
                }
            }
        }

        Ok(parts.join("\n\n"))
    }

    pub fn append_to_prompt(&self, system_prompt: &str, query: Option<&str>) -> String {
        match self.build_inject_block(query) {
            Ok(block) if !block.trim().is_empty() => {
                format!("{}\n\n{}", system_prompt.trim_end(), block)
            }
            _ => system_prompt.to_string(),
        }
    }
}

fn resolve_id(db: &Database, bank_key: &str, id_or_prefix: &str) -> anyhow::Result<Option<String>> {
    let id_or_prefix = id_or_prefix.trim();
    if id_or_prefix.is_empty() {
        return Ok(None);
    }
    if let Some(row) = db.get_memory(id_or_prefix)?
        && row.project_key == bank_key
    {
        return Ok(Some(row.id));
    }
    let rows = db.list_memories(bank_key, 10_000)?;
    let matches: Vec<_> = rows
        .into_iter()
        .filter(|r| r.id.starts_with(id_or_prefix))
        .collect();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(Some(matches[0].id.clone())),
        _ => anyhow::bail!(
            "ambiguous memory id prefix '{id_or_prefix}' matches {} entries",
            matches.len()
        ),
    }
}

pub fn settings_from_flags(enabled: bool) -> MemorySettings {
    if enabled {
        MemorySettings::default()
    } else {
        MemorySettings::disabled()
    }
}

pub fn apply_memory_prompt(
    system_prompt: &str,
    project_path: &Path,
    data_dir: &Path,
    settings: &MemorySettings,
    query: Option<&str>,
) -> String {
    if !settings.enabled {
        return system_prompt.to_string();
    }
    match MemoryService::open(project_path, data_dir, settings.clone()) {
        Ok(svc) => svc.append_to_prompt(system_prompt, query),
        Err(e) => {
            tracing::warn!("memory inject skipped: {e}");
            system_prompt.to_string()
        }
    }
}

/// Best-effort post-turn retain for CLI/TUI (heuristic only).
pub fn maybe_auto_retain(
    project_path: &Path,
    data_dir: &Path,
    settings: &MemorySettings,
    user_text: &str,
    assistant_text: Option<&str>,
    session_id: Option<&str>,
    turn_index: usize,
) -> Vec<String> {
    if !settings.enabled || !settings.auto_retain {
        return Vec::new();
    }
    match MemoryService::open(project_path, data_dir, settings.clone()) {
        Ok(svc) => svc
            .auto_retain(user_text, assistant_text, session_id, turn_index)
            .unwrap_or_default(),
        Err(e) => {
            tracing::debug!("auto_retain skipped: {e}");
            Vec::new()
        }
    }
}

/// Best-effort: build code index if empty. Returns chunks indexed, if any.
pub fn maybe_auto_index(
    project_path: &Path,
    data_dir: &Path,
    settings: &MemorySettings,
) -> Option<usize> {
    if !settings.enabled || !settings.auto_index {
        return None;
    }
    match MemoryService::open(project_path, data_dir, settings.clone()) {
        Ok(svc) => match svc.ensure_code_index() {
            Ok(n) => n,
            Err(e) => {
                tracing::debug!("auto_index skipped: {e}");
                None
            }
        },
        Err(e) => {
            tracing::debug!("auto_index open failed: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn remember_search_delete() {
        let dir = tempdir().unwrap();
        let data = dir.path().join("data");
        let project = dir.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();

        let svc = MemoryService::open(&project, &data, MemorySettings::default()).unwrap();
        let id = svc
            .remember("always run cargo test -p whycode-memory after edits", None)
            .unwrap();
        assert!(!id.is_empty());

        let hits = svc
            .search("how do I test the memory crate", 3, 0.1)
            .unwrap();
        assert!(!hits.is_empty());
        assert!(hits[0].entry.text.contains("cargo test"));

        assert!(svc.delete(&id[..8]).unwrap());
        assert!(svc.search("cargo test memory", 3, 0.1).unwrap().is_empty());
    }

    #[test]
    fn auto_retain_from_user_preference() {
        let dir = tempdir().unwrap();
        let data = dir.path().join("data");
        let project = dir.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let svc = MemoryService::open(&project, &data, MemorySettings::default()).unwrap();
        let saved = svc
            .auto_retain(
                "Always prefer fish shell for scripts in this repo.",
                None,
                None,
                1,
            )
            .unwrap();
        assert!(!saved.is_empty());
        assert!(!svc.list(10).unwrap().is_empty());
    }

    #[test]
    fn agent_bank_isolated() {
        let dir = tempdir().unwrap();
        let data = dir.path().join("data");
        let project = dir.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let main = MemorySettings {
            agent_bank: None,
            ..Default::default()
        };
        let explore = MemorySettings {
            agent_bank: Some("explore".into()),
            ..Default::default()
        };
        let s_main = MemoryService::open(&project, &data, main).unwrap();
        let s_ex = MemoryService::open(&project, &data, explore).unwrap();
        s_main
            .remember("main bank fact unique alpha", None)
            .unwrap();
        s_ex.remember("explore bank fact unique beta", None)
            .unwrap();
        assert_eq!(s_main.list(10).unwrap().len(), 1);
        assert_eq!(s_ex.list(10).unwrap().len(), 1);
        assert!(s_main.list(10).unwrap()[0].text.contains("alpha"));
        assert!(s_ex.list(10).unwrap()[0].text.contains("beta"));
    }

    #[test]
    fn project_scope_writes_under_whycode() {
        let dir = tempdir().unwrap();
        let data = dir.path().join("data");
        let project = dir.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let mut settings = MemorySettings::default();
        settings.scope = crate::settings::MemoryScope::Project;
        let svc = MemoryService::open(&project, &data, settings).unwrap();
        svc.remember("project scoped memory item", None).unwrap();
        assert!(svc.memory_md_path().starts_with(project.join(".whycode")));
        assert!(svc.memory_md_path().exists());
    }

    #[test]
    fn disabled_returns_empty_inject() {
        let dir = tempdir().unwrap();
        let data = dir.path().join("data");
        let project = dir.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let svc = MemoryService::open(&project, &data, MemorySettings::disabled()).unwrap();
        assert!(svc.build_inject_block(Some("x")).unwrap().is_empty());
        assert!(svc.remember("nope", None).is_err());
    }
}
