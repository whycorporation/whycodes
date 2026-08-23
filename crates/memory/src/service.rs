//! High-level memory API: dual-write SQLite + MEMORY.md, inject, retain.

use std::path::{Path, PathBuf};

use whycode_storage::db::Database;
use whycode_storage::models::{MemoryRow, SessionChunkRow};

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

/// A scored past-session turn hit.
#[derive(Debug, Clone)]
pub struct SessionHit {
    pub entry: SessionChunkRow,
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

    /// Embed this turn and store it for later session search.
    pub fn index_session_turn(
        &self,
        session_id: &str,
        turn_index: usize,
        user_text: &str,
        assistant_text: &str,
    ) -> anyhow::Result<()> {
        if !self.settings.enabled {
            return Ok(());
        }
        let mut clip = String::new();
        if !user_text.trim().is_empty() {
            clip.push_str("User: ");
            clip.push_str(user_text.trim());
            clip.push('\n');
        }
        if !assistant_text.trim().is_empty() {
            clip.push_str("Assistant: ");
            clip.push_str(assistant_text.trim());
        }
        if clip.trim().is_empty() {
            return Ok(());
        }
        const MAX: usize = 2000;
        if clip.len() > MAX {
            // Byte cap can land inside a multibyte char; String::truncate panics.
            clip.truncate(clip.floor_char_boundary(MAX));
        }
        let vec = self.embed_text(&clip);
        let blob = encode_blob(&vec);
        let id = uuid::Uuid::new_v4().to_string();
        let db = self.open_db()?;
        db.insert_session_chunk(
            &id,
            &self.bank_key,
            session_id,
            turn_index as i64,
            &clip,
            &blob,
        )?;
        Ok(())
    }

    /// Semantic search over prior session turns.
    pub fn search_sessions(
        &self,
        query: &str,
        top_k: usize,
        min_score: f32,
    ) -> anyhow::Result<Vec<SessionHit>> {
        let db = self.open_db()?;
        let rows = db.list_session_chunks(&self.bank_key, 10_000)?;
        let q = self.embed_text(query);
        let mut hits: Vec<SessionHit> = rows
            .into_iter()
            .filter_map(|entry| {
                let v = decode_blob(&entry.embedding);
                if v.is_empty() || v.len() != q.len() {
                    return None;
                }
                let score = cosine(&q, &v);
                if score >= min_score {
                    Some(SessionHit { entry, score })
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

    /// Drop oldest unused facts when the bank is over `consolidate_max`.
    pub fn consolidate(&self) -> anyhow::Result<usize> {
        if !self.settings.enabled || !self.settings.consolidate {
            return Ok(0);
        }
        let db = self.open_db()?;
        let cap = self.settings.consolidate_max.max(1);
        let rows = db.list_memories(&self.bank_key, 10_000)?;
        if rows.len() <= cap {
            return Ok(0);
        }
        let mut ranked = rows;
        ranked.sort_by(|a, b| {
            a.recall_count
                .cmp(&b.recall_count)
                .then_with(|| a.created_at.cmp(&b.created_at))
        });
        let overflow = ranked.len() - cap;
        let mut dropped = 0usize;
        for row in ranked.into_iter().take(overflow) {
            if db.delete_memory(&row.id)? {
                let short = &row.id[..8.min(row.id.len())];
                if let Err(e) = markdown::remove_entry(&self.memory_md_path(), short) {
                    tracing::debug!(id = %short, error = %e, "MEMORY.md remove after consolidate");
                }
                dropped += 1;
            }
        }
        Ok(dropped)
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

            if self.settings.session_inject
                && let Ok(sess_hits) = self.search_sessions(
                    q,
                    self.settings.session_top_k,
                    self.settings.session_min_score,
                )
                && !sess_hits.is_empty()
            {
                let mut lines = Vec::new();
                let mut used = 0usize;
                let header = "# Past sessions (related turns; verify if stale)\n";
                used += header.len();
                for hit in &sess_hits {
                    let snippet = hit
                        .entry
                        .text
                        .lines()
                        .take(6)
                        .collect::<Vec<_>>()
                        .join("\n");
                    let sid = &hit.entry.session_id;
                    let short = &sid[..8.min(sid.len())];
                    let line = format!(
                        "- [{:.2}] session {short} turn {}\n{}\n",
                        hit.score, hit.entry.turn_index, snippet
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

    fn open_test_service(
        settings: MemorySettings,
    ) -> (tempfile::TempDir, PathBuf, PathBuf, MemoryService) {
        let dir = tempdir().unwrap();
        let data = dir.path().join("data");
        let project = dir.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let svc = MemoryService::open(&project, &data, settings).unwrap();
        (dir, data, project, svc)
    }

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
    fn session_turn_search_and_inject() {
        let dir = tempdir().unwrap();
        let data = dir.path().join("data");
        let project = dir.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let svc = MemoryService::open(&project, &data, MemorySettings::default()).unwrap();
        svc.index_session_turn(
            "sess-aaa",
            1,
            "How do we run the retry loop?",
            "The retry loop lives in crates/llm/src/retry.rs.",
        )
        .unwrap();
        let hits = svc.search_sessions("retry loop", 3, 0.05).unwrap();
        assert!(!hits.is_empty());
        assert!(hits[0].entry.text.contains("retry"));
        let block = svc.build_inject_block(Some("retry loop")).unwrap();
        assert!(block.contains("Past sessions"), "{block}");
    }

    #[test]
    fn consolidate_drops_overflow() {
        let dir = tempdir().unwrap();
        let data = dir.path().join("data");
        let project = dir.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let settings = MemorySettings {
            consolidate: true,
            consolidate_max: 2,
            ..Default::default()
        };
        let svc = MemoryService::open(&project, &data, settings).unwrap();
        svc.remember("fact alpha unique zebra", None).unwrap();
        svc.remember("fact beta unique yak", None).unwrap();
        svc.remember("fact gamma unique xylophone", None).unwrap();
        assert_eq!(svc.list(10).unwrap().len(), 3);
        let dropped = svc.consolidate().unwrap();
        assert_eq!(dropped, 1);
        assert_eq!(svc.list(10).unwrap().len(), 2);
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

    #[test]
    fn remember_rejects_empty_and_duplicate_text() {
        let (_dir, _data, _project, svc) = open_test_service(MemorySettings::default());
        assert_eq!(
            svc.remember("  \n ", None).unwrap_err().to_string(),
            "memory text is empty"
        );

        svc.remember("use deterministic local memory tests", Some("session-a"))
            .unwrap();
        let error = svc
            .remember("  use deterministic local memory tests  ", None)
            .unwrap_err();
        assert!(error.to_string().contains("duplicate memory"));
        assert_eq!(svc.list(10).unwrap().len(), 1);
        assert_eq!(
            svc.list(10).unwrap()[0].source_session.as_deref(),
            Some("session-a")
        );
    }

    #[test]
    fn delete_handles_missing_full_and_ambiguous_ids() {
        let (_dir, _data, _project, svc) = open_test_service(MemorySettings::default());
        let db = svc.open_db().unwrap();
        let embedding = encode_blob(&svc.embed_text("shared prefix fact"));
        db.insert_memory("shared-a", &svc.bank_key, "first", &embedding, None)
            .unwrap();
        db.insert_memory("shared-b", &svc.bank_key, "second", &embedding, None)
            .unwrap();
        db.insert_memory("other-bank", "unrelated", "third", &embedding, None)
            .unwrap();

        assert!(!svc.delete("").unwrap());
        assert!(!svc.delete("missing").unwrap());
        assert!(
            svc.delete("shared")
                .unwrap_err()
                .to_string()
                .contains("ambiguous")
        );
        assert!(svc.delete("shared-a").unwrap());
        assert_eq!(svc.list(10).unwrap().len(), 1);
    }

    #[test]
    fn clear_removes_only_current_bank_and_resets_markdown() {
        let (_dir, data, project, svc) = open_test_service(MemorySettings::default());
        svc.remember("main bank clear target", None).unwrap();
        let other = MemoryService::open(
            project,
            data,
            MemorySettings {
                agent_bank: Some("other".into()),
                ..Default::default()
            },
        )
        .unwrap();
        other.remember("other bank survives clear", None).unwrap();

        assert_eq!(svc.clear().unwrap(), 1);
        assert!(svc.list(10).unwrap().is_empty());
        assert_eq!(other.list(10).unwrap().len(), 1);
        assert_eq!(
            std::fs::read_to_string(svc.memory_md_path()).unwrap(),
            "# Whycode auto memory\n\n"
        );
    }

    #[test]
    fn search_skips_invalid_embeddings_and_keeps_one_when_top_k_is_zero() {
        let (_dir, _data, _project, svc) = open_test_service(MemorySettings::default());
        let db = svc.open_db().unwrap();
        db.insert_memory("empty", &svc.bank_key, "empty vector", &[], None)
            .unwrap();
        db.insert_memory(
            "wrong-dim",
            &svc.bank_key,
            "wrong vector",
            &[0, 0, 0, 0],
            None,
        )
        .unwrap();
        svc.remember("matching vector fact", None).unwrap();

        let hits = svc.search("matching vector fact", 0, -1.0).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.text, "matching vector fact");
        assert!(svc.search("anything", 5, 2.0).unwrap().is_empty());
    }

    #[test]
    fn session_index_ignores_disabled_and_blank_turns_and_clips_long_text() {
        let (_dir, _data, _project, disabled) = open_test_service(MemorySettings::disabled());
        disabled
            .index_session_turn("disabled", 0, "user", "assistant")
            .unwrap();
        assert!(
            disabled
                .search_sessions("user", 5, -1.0)
                .unwrap()
                .is_empty()
        );

        let (_dir, _data, _project, svc) = open_test_service(MemorySettings::default());
        svc.index_session_turn("blank", 0, " \n", "\t").unwrap();
        svc.index_session_turn("long-session", 3, &"x".repeat(2500), "tail")
            .unwrap();
        // "User: " (6) + 1993 ASCII + 2-byte `ç` + `\n` = 2002. Byte 2000 sits
        // inside `ç`; String::truncate(2000) used to panic (crash-20260823).
        svc.index_session_turn("utf8-session", 4, &format!("{}ç", "a".repeat(1993)), "")
            .unwrap();
        let rows = svc
            .open_db()
            .unwrap()
            .list_session_chunks(&svc.bank_key, 10)
            .unwrap();
        assert_eq!(rows.len(), 2);
        let ascii = rows
            .iter()
            .find(|r| r.session_id == "long-session")
            .unwrap();
        assert_eq!(ascii.text.len(), 2000);
        assert!(ascii.text.starts_with("User: "));
        let utf8 = rows
            .iter()
            .find(|r| r.session_id == "utf8-session")
            .unwrap();
        assert!(utf8.text.is_char_boundary(utf8.text.len()));
        assert!(utf8.text.len() <= 2000);
        assert!(
            !utf8.text.contains('ç'),
            "multibyte tail must be dropped rather than splitting the char"
        );
    }

    #[test]
    fn session_search_skips_invalid_vectors_and_honors_minimum_result_limit() {
        let (_dir, _data, _project, svc) = open_test_service(MemorySettings::default());
        let db = svc.open_db().unwrap();
        db.insert_session_chunk("bad", &svc.bank_key, "s", 0, "bad", &[])
            .unwrap();
        svc.index_session_turn("good", 1, "local sqlite state", "stored")
            .unwrap();

        let hits = svc.search_sessions("local sqlite state", 0, -1.0).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.session_id, "good");
    }

    #[test]
    fn retain_scheduling_and_limits_cover_skip_and_llm_paths() {
        let settings = MemorySettings {
            retain_every_n: 3,
            retain_max_facts: 1,
            ..Default::default()
        };
        let (_dir, _data, _project, svc) = open_test_service(settings);
        assert!(
            svc.auto_retain("Always use local tests.", None, None, 2)
                .unwrap()
                .is_empty()
        );
        assert!(!svc.should_run_llm_retain(0, 2));
        assert!(svc.should_run_llm_retain(0, 3));
        assert!(!svc.should_run_llm_retain(1, 3));

        let saved = svc
            .retain_llm_facts("- first retained fact\n- second retained fact", None)
            .unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(svc.list(10).unwrap().len(), 1);
    }

    #[test]
    fn consolidate_noops_when_disabled_or_within_capacity() {
        let (_dir, _data, _project, disabled) = open_test_service(MemorySettings::disabled());
        assert_eq!(disabled.consolidate().unwrap(), 0);

        let settings = MemorySettings {
            consolidate: true,
            consolidate_max: 0,
            ..Default::default()
        };
        let (_dir, _data, _project, svc) = open_test_service(settings);
        svc.remember("single retained item", None).unwrap();
        assert_eq!(svc.consolidate().unwrap(), 0);
        assert_eq!(svc.list(10).unwrap().len(), 1);
    }

    #[test]
    fn inject_and_append_cover_index_recall_and_blank_query() {
        let settings = MemorySettings {
            code_inject: false,
            session_inject: false,
            recall_min_score: -1.0,
            ..Default::default()
        };
        let (_dir, _data, _project, svc) = open_test_service(settings);
        svc.remember("inject this local fact", None).unwrap();

        let index_only = svc.build_inject_block(Some("  ")).unwrap();
        assert!(index_only.contains("# Auto Memory"));
        assert!(!index_only.contains("# Recalled Memories"));
        let prompt = svc.append_to_prompt("system  \n", Some("local fact"));
        assert!(prompt.starts_with("system\n\n# Auto Memory"));
        assert!(prompt.contains("# Recalled Memories"));
        assert_eq!(
            MemoryService::open(
                svc.project_path.clone(),
                svc.data_dir.clone(),
                MemorySettings::disabled()
            )
            .unwrap()
            .append_to_prompt("unchanged", Some("query")),
            "unchanged"
        );
    }

    #[test]
    fn code_index_skips_disabled_and_existing_local_index() {
        let (_dir, _data, _project, disabled) = open_test_service(MemorySettings::disabled());
        assert_eq!(disabled.ensure_code_index().unwrap(), None);

        let (_dir, _data, _project, svc) = open_test_service(MemorySettings::default());
        svc.open_db()
            .unwrap()
            .insert_code_chunk(
                "chunk",
                &svc.bank_key,
                "src/lib.rs",
                1,
                1,
                "fn local() {}",
                &[],
            )
            .unwrap();
        assert_eq!(svc.ensure_code_index().unwrap(), None);
    }

    #[test]
    fn best_effort_wrappers_handle_disabled_and_open_failures() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let blocked_data = dir.path().join("not-a-directory");
        std::fs::write(&blocked_data, "file").unwrap();
        let enabled = MemorySettings::default();
        let disabled = MemorySettings::disabled();

        assert!(!settings_from_flags(false).enabled);
        assert!(settings_from_flags(true).enabled);
        assert_eq!(
            apply_memory_prompt("base", &project, &blocked_data, &enabled, Some("q")),
            "base"
        );
        assert_eq!(
            apply_memory_prompt("base", &project, &blocked_data, &disabled, Some("q")),
            "base"
        );
        assert!(
            maybe_auto_retain(
                &project,
                &blocked_data,
                &enabled,
                "Always test locally.",
                None,
                None,
                1
            )
            .is_empty()
        );
        assert!(maybe_auto_index(&project, &blocked_data, &enabled).is_none());
        assert!(maybe_auto_index(&project, &blocked_data, &disabled).is_none());
    }

    #[test]
    fn disabled_mutations_and_retention_are_rejected_or_skipped() {
        let (_dir, _data, _project, svc) = open_test_service(MemorySettings::disabled());
        assert!(
            svc.delete("id")
                .unwrap_err()
                .to_string()
                .contains("disabled")
        );
        assert!(svc.clear().unwrap_err().to_string().contains("disabled"));
        assert!(
            svc.auto_retain("Always remember this.", None, None, 1)
                .unwrap()
                .is_empty()
        );
        assert!(svc.retain_llm_facts("- fact", None).unwrap().is_empty());
        assert!(!svc.should_run_llm_retain(0, 1));
    }
}
