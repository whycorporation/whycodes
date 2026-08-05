//! High-level memory API: dual-write SQLite + MEMORY.md, inject blocks.

use std::path::{Path, PathBuf};

use whycode_storage::db::Database;
use whycode_storage::models::MemoryRow;

use crate::embed::{cosine, decode_blob, embed, encode_blob};
use crate::markdown;
use crate::paths::{ensure_memory_dir, memory_md};
use crate::project_key::project_key;
use crate::settings::MemorySettings;

/// A scored recall hit.
#[derive(Debug, Clone)]
pub struct RecallHit {
    pub entry: MemoryRow,
    pub score: f32,
}

/// Project-scoped memory service.
pub struct MemoryService {
    pub project_key: String,
    pub project_path: PathBuf,
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub settings: MemorySettings,
}

impl MemoryService {
    /// Open for `project_path` using `data_dir` (typically `Config::data_dir()`).
    pub fn open(
        project_path: impl Into<PathBuf>,
        data_dir: impl Into<PathBuf>,
        settings: MemorySettings,
    ) -> anyhow::Result<Self> {
        let project_path = project_path.into();
        let data_dir = data_dir.into();
        let key = project_key(&project_path);
        let db_path = data_dir.join("whycode.db");
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        ensure_memory_dir(&data_dir, &project_path)?;
        Ok(Self {
            project_key: key,
            project_path,
            data_dir,
            db_path,
            settings,
        })
    }

    pub fn memory_md_path(&self) -> PathBuf {
        memory_md(&self.data_dir, &self.project_path)
    }

    fn open_db(&self) -> anyhow::Result<Database> {
        Database::open(self.db_path.to_str().unwrap_or("whycode.db"))
    }

    /// Store a durable fact. Returns the new id.
    pub fn remember(&self, text: &str, source_session: Option<&str>) -> anyhow::Result<String> {
        if !self.settings.enabled {
            anyhow::bail!("memory is disabled");
        }
        let text = text.trim();
        if text.is_empty() {
            anyhow::bail!("memory text is empty");
        }
        let id = uuid::Uuid::new_v4().to_string();
        let short_id = &id[..8.min(id.len())];
        let vec = embed(text, self.settings.embed_dim);
        let blob = encode_blob(&vec);
        let db = self.open_db()?;
        db.insert_memory(
            &id,
            &self.project_key,
            text,
            &blob,
            source_session,
        )?;
        markdown::append_entry(&self.memory_md_path(), short_id, text)?;
        // Store short id mapping: we keep full UUID in SQLite; MEMORY.md uses short prefix.
        // For delete-by-short-id we match prefix.
        Ok(id)
    }

    pub fn list(&self, limit: usize) -> anyhow::Result<Vec<MemoryRow>> {
        let db = self.open_db()?;
        db.list_memories(&self.project_key, limit)
    }

    pub fn delete(&self, id_or_prefix: &str) -> anyhow::Result<bool> {
        if !self.settings.enabled {
            anyhow::bail!("memory is disabled");
        }
        let db = self.open_db()?;
        let id = resolve_id(&db, &self.project_key, id_or_prefix)?;
        let Some(id) = id else {
            return Ok(false);
        };
        let removed = db.delete_memory(&id)?;
        if removed {
            let short = &id[..8.min(id.len())];
            let _ = markdown::remove_entry(&self.memory_md_path(), short);
            // Also try full id in case older format
            let _ = markdown::remove_entry(&self.memory_md_path(), &id);
        }
        Ok(removed)
    }

    pub fn clear(&self) -> anyhow::Result<usize> {
        if !self.settings.enabled {
            anyhow::bail!("memory is disabled");
        }
        let db = self.open_db()?;
        let n = db.clear_memories(&self.project_key)?;
        markdown::clear_file(&self.memory_md_path())?;
        Ok(n)
    }

    /// Semantic search over stored facts.
    pub fn search(&self, query: &str, top_k: usize, min_score: f32) -> anyhow::Result<Vec<RecallHit>> {
        let db = self.open_db()?;
        let rows = db.list_memories(&self.project_key, 10_000)?;
        let q = embed(query, self.settings.embed_dim);
        let mut hits: Vec<RecallHit> = rows
            .into_iter()
            .filter_map(|entry| {
                let v = decode_blob(&entry.embedding);
                if v.is_empty() {
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
        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(top_k.max(1));
        Ok(hits)
    }

    /// Build the system-prompt injection block (empty if disabled / nothing to show).
    pub fn build_inject_block(&self, query: Option<&str>) -> anyhow::Result<String> {
        if !self.settings.enabled {
            return Ok(String::new());
        }

        let mut parts: Vec<String> = Vec::new();
        let mut char_budget = self.settings.recall_char_budget()
            + self.settings.max_index_bytes.min(self.settings.recall_char_budget() * 2);

        // Always-on MEMORY.md index (Claude-style)
        let index = markdown::load_capped(
            &self.memory_md_path(),
            self.settings.max_index_lines,
            self.settings.max_index_bytes,
        );
        if !index.is_empty() {
            let block = format!(
                "# Auto Memory\n\n\
                 Notes the agent saved for this project (machine-local). Prefer repo truth if stale.\n\n\
                 {index}"
            );
            char_budget = char_budget.saturating_sub(block.len());
            parts.push(block);
        }

        // Semantic recall for the current user query (Grok/jcode-style auto-recall)
        if self.settings.auto_inject {
            if let Some(q) = query.map(str::trim).filter(|s| !s.is_empty()) {
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
                        // Best-effort recall stats
                        if let Ok(db) = self.open_db() {
                            for id in ids {
                                let _ = db.touch_memory_recall(&id);
                            }
                        }
                    }
                }
            }
        }

        Ok(parts.join("\n\n"))
    }

    /// Append memory block to a system prompt.
    pub fn append_to_prompt(&self, system_prompt: &str, query: Option<&str>) -> String {
        match self.build_inject_block(query) {
            Ok(block) if !block.trim().is_empty() => {
                format!("{}\n\n{}", system_prompt.trim_end(), block)
            }
            _ => system_prompt.to_string(),
        }
    }
}

fn resolve_id(db: &Database, project_key: &str, id_or_prefix: &str) -> anyhow::Result<Option<String>> {
    let id_or_prefix = id_or_prefix.trim();
    if id_or_prefix.is_empty() {
        return Ok(None);
    }
    if let Some(row) = db.get_memory(id_or_prefix)? {
        if row.project_key == project_key {
            return Ok(Some(row.id));
        }
    }
    // Prefix match
    let rows = db.list_memories(project_key, 10_000)?;
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

/// Convenience: build inject settings from common knobs (CLI/TUI).
pub fn settings_from_flags(enabled: bool) -> MemorySettings {
    if enabled {
        MemorySettings::default()
    } else {
        MemorySettings::disabled()
    }
}

/// Apply memory to a system prompt when enabled.
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
        assert!(!hits.is_empty(), "expected a hit");
        assert!(hits[0].entry.text.contains("cargo test"));

        let weather = svc.search("weather forecast for paris", 3, 0.35).unwrap();
        // High threshold should exclude unrelated
        assert!(
            weather.is_empty() || weather[0].score < hits[0].score,
            "unrelated should not outrank"
        );

        assert!(svc.delete(&id[..8]).unwrap());
        let hits = svc.search("cargo test memory", 3, 0.1).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn inject_includes_auto_memory() {
        let dir = tempdir().unwrap();
        let data = dir.path().join("data");
        let project = dir.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let svc = MemoryService::open(&project, &data, MemorySettings::default()).unwrap();
        svc.remember("prefer fish shell for scripts", None).unwrap();
        let block = svc.build_inject_block(Some("what shell should I use")).unwrap();
        assert!(block.contains("Auto Memory") || block.contains("Recalled"));
        assert!(block.contains("fish") || block.contains("shell"));
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
