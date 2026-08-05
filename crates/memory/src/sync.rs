//! Cross-machine sync helpers: export / import JSON snapshots.

use serde::{Deserialize, Serialize};

use crate::service::MemoryService;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryExport {
    pub version: u32,
    pub project_key: String,
    pub bank_key: String,
    pub exported_at: String,
    pub entries: Vec<MemoryExportEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryExportEntry {
    pub id: String,
    pub text: String,
    pub created_at: String,
    pub source_session: Option<String>,
}

impl MemoryService {
    /// Export all facts for this bank as JSON (portable across machines).
    pub fn export_json(&self) -> anyhow::Result<String> {
        let rows = self.list(50_000)?;
        let exp = MemoryExport {
            version: 1,
            project_key: self.project_key.clone(),
            bank_key: self.bank_key.clone(),
            exported_at: chrono::Utc::now().to_rfc3339(),
            entries: rows
                .into_iter()
                .map(|r| MemoryExportEntry {
                    id: r.id,
                    text: r.text,
                    created_at: r.created_at,
                    source_session: r.source_session,
                })
                .collect(),
        };
        Ok(serde_json::to_string_pretty(&exp)?)
    }

    /// Import facts from export JSON. Re-embeds on import. Returns (added, skipped).
    pub fn import_json(&self, json: &str) -> anyhow::Result<(usize, usize)> {
        let exp: MemoryExport = serde_json::from_str(json)?;
        let mut added = 0usize;
        let mut skipped = 0usize;
        for e in exp.entries {
            // Dedupe by high similarity to existing
            if self.is_duplicate(&e.text)? {
                skipped += 1;
                continue;
            }
            match self.remember(&e.text, e.source_session.as_deref()) {
                Ok(_) => added += 1,
                Err(_) => skipped += 1,
            }
        }
        Ok((added, skipped))
    }
}
