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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryService;
    use crate::settings::MemorySettings;
    use tempfile::tempdir;

    fn open_svc() -> (tempfile::TempDir, MemoryService) {
        let dir = tempdir().unwrap();
        let data = dir.path().join("data");
        let project = dir.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let svc = MemoryService::open(&project, &data, MemorySettings::default()).unwrap();
        (dir, svc)
    }

    #[test]
    fn export_roundtrip_imports_new_facts() {
        let (_dir, src) = open_svc();
        src.remember("always use cargo test after rust edits", Some("s1"))
            .unwrap();
        let json = src.export_json().unwrap();
        let parsed: MemoryExport = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(
            parsed.entries[0].text,
            "always use cargo test after rust edits"
        );
        assert_eq!(parsed.entries[0].source_session.as_deref(), Some("s1"));

        let (_dir2, dst) = open_svc();
        let (added, skipped) = dst.import_json(&json).unwrap();
        assert_eq!(added, 1);
        assert_eq!(skipped, 0);
        assert_eq!(dst.list(10).unwrap().len(), 1);
    }

    #[test]
    fn import_skips_duplicates() {
        let (_dir, svc) = open_svc();
        svc.remember("prefer fish shell in this repo", None)
            .unwrap();
        let json = svc.export_json().unwrap();
        let (added, skipped) = svc.import_json(&json).unwrap();
        assert_eq!(added, 0);
        assert_eq!(skipped, 1);
        assert_eq!(svc.list(10).unwrap().len(), 1);
    }

    #[test]
    fn import_rejects_garbage() {
        let (_dir, svc) = open_svc();
        assert!(svc.import_json("not-json").is_err());
    }
}
