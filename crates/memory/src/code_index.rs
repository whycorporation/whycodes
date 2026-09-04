//! Lightweight codebase RAG (hash embeddings over source chunks).

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::embed::{cosine, decode_blob, encode_blob};
use crate::error::{MemoryError, Result};
use crate::service::{CodeHit, MemoryService};

const EXT_OK: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "py", "go", "java", "kt", "c", "h", "cpp", "hpp", "cs", "rb",
    "php", "swift", "md", "toml", "yaml", "yml", "json", "sql", "sh", "fish", "css", "html", "vue",
    "svelte",
];

impl MemoryService {
    /// Walk the project, chunk text files, embed, replace prior index for this bank.
    pub fn index_codebase(&self, max_files: usize, max_chunks: usize) -> Result<usize> {
        if !self.settings.enabled {
            return Err(MemoryError::msg("memory is disabled"));
        }
        let root = crate::project_key::project_root(&self.project_path);
        let mut files = Vec::new();
        walk_files(&root, &mut files, max_files);
        files.sort();

        let db = self.open_db()?;
        db.clear_code_chunks(&self.bank_key)?;

        let mut n = 0usize;
        for rel in files {
            if n >= max_chunks {
                break;
            }
            let abs = root.join(&rel);
            let Ok(content) = std::fs::read_to_string(&abs) else {
                continue;
            };
            if content.len() > 400_000 {
                continue;
            }
            for (start, end, text) in chunk_source(&content, 40, 10) {
                if n >= max_chunks {
                    break;
                }
                let text = text.trim();
                if text.len() < 40 {
                    continue;
                }
                // Prefix path so search matches file-oriented queries
                let indexed = format!("// {rel}:{start}-{end}\n{text}");
                let vec = self.embed_text(&indexed);
                let blob = encode_blob(&vec);
                let id = uuid::Uuid::new_v4().to_string();
                put_chunk(&db, &id, &self.bank_key, &rel, start, end, text, &blob)?;
                n += 1;
            }
        }
        Ok(n)
    }

    /// Semantic search over indexed code chunks.
    pub fn search_code(&self, query: &str, top_k: usize, min_score: f32) -> Result<Vec<CodeHit>> {
        let db = self.open_db()?;
        let rows = db.list_code_chunks(&self.bank_key, 50_000)?;
        let q = self.embed_text(query);
        let mut hits: Vec<CodeHit> = rows
            .into_iter()
            .filter_map(|entry| {
                let v = decode_blob(&entry.embedding);
                if v.is_empty() {
                    return None;
                }
                let score = cosine(&q, &v);
                if score >= min_score {
                    Some(CodeHit { entry, score })
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
}

#[allow(clippy::too_many_arguments)]
fn put_chunk(
    db: &whycodes_storage::db::Database,
    id: &str,
    bank_key: &str,
    rel: &str,
    start: usize,
    end: usize,
    text: &str,
    blob: &[u8],
) -> Result<()> {
    Ok(db.insert_code_chunk(id, bank_key, rel, start as i64, end as i64, text, blob)?)
}

/// Collect indexable source files under `root` via the shared workspace
/// walker (gitignore-aware, policy-pruned — see `whycodes_index::walk`).
fn walk_files(root: &Path, out: &mut Vec<String>, max_files: usize) {
    let scanned = AtomicUsize::new(0);
    let cancel = AtomicBool::new(false);
    let collected = Mutex::new(std::mem::take(out));
    whycodes_index::walk::walk_root(root, 4, usize::MAX, &scanned, &cancel, &|e| {
        if e.is_dir {
            return;
        }
        let ext = e.rel.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        if !EXT_OK.contains(&ext.as_str()) {
            return;
        }
        let mut g = crate::recover_lock(&collected);
        if g.len() >= max_files {
            cancel.store(true, Ordering::Relaxed);
            return;
        }
        g.push(e.rel.to_string());
    });
    *out = crate::recover_mutex(collected);
}

/// Sliding windows of `window` lines with `overlap` lines shared.
fn chunk_source(content: &str, window: usize, overlap: usize) -> Vec<(usize, usize, String)> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let step = window.saturating_sub(overlap).max(1);
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let end = (i + window).min(lines.len());
        let start_line = i + 1;
        let end_line = end;
        let text = lines[i..end].join("\n");
        out.push((start_line, end_line, text));
        if end >= lines.len() {
            break;
        }
        i += step;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::MemorySettings;
    use tempfile::tempdir;

    #[test]
    fn index_and_search_rust() {
        let dir = tempdir().unwrap();
        let data = dir.path().join("data");
        let project = dir.path().join("proj");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(
            project.join("src/lib.rs"),
            "/// Memory service for semantic recall.\npub fn remember_fact(s: &str) {}\n",
        )
        .unwrap();
        let svc = MemoryService::open(&project, &data, MemorySettings::default()).unwrap();
        let n = svc.index_codebase(100, 500).unwrap();
        assert!(n >= 1);
        let hits = svc
            .search_code("semantic recall remember", 3, 0.05)
            .unwrap();
        assert!(!hits.is_empty());
        assert!(hits[0].entry.path.contains("lib.rs"));
    }

    #[test]
    fn index_skips_disabled_unreadable_huge_short_and_limits() {
        let dir = tempdir().unwrap();
        let data = dir.path().join("data");
        let project = dir.path().join("proj");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(project.join("src/skip.bin"), "not source").unwrap();
        std::fs::write(project.join("src/empty.rs"), "").unwrap();
        std::fs::write(project.join("src/tiny.rs"), "fn t() {}\n").unwrap();
        std::fs::write(project.join("src/huge.rs"), "a".repeat(400_001)).unwrap();
        std::fs::create_dir_all(project.join("src/dir.rs")).unwrap();
        let long: String = (0..90)
            .map(|i| format!("fn item_{i}() {{ /* body */ }}\n"))
            .collect();
        std::fs::write(project.join("src/lib.rs"), &long).unwrap();
        std::fs::write(project.join("src/other.rs"), &long).unwrap();

        let disabled = MemoryService::open(
            &project,
            &data,
            MemorySettings {
                enabled: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            disabled
                .index_codebase(10, 10)
                .unwrap_err()
                .to_string()
                .contains("disabled")
        );

        let svc = MemoryService::open(&project, &data, MemorySettings::default()).unwrap();
        let denied = project.join("src/denied.rs");
        std::fs::write(
            &denied,
            "fn denied_source_file_is_unreadable_on_purpose() {}\n",
        )
        .unwrap();
        let mut perm = std::fs::metadata(&denied).unwrap().permissions();
        perm.set_readonly(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o000)).unwrap();
        }
        let n = svc.index_codebase(10, 1).unwrap();
        assert_eq!(n, 1);
        let ranked = svc.search_code("item_1", 8, -1.0).unwrap();
        assert!(!ranked.is_empty());
        assert_eq!(svc.index_codebase(0, 10).unwrap(), 0);
        let _ = svc.index_codebase(1, 500);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o644)).unwrap();
        }

        let db = svc.open_db().unwrap();
        db.insert_code_chunk("empty-emb", &svc.bank_key, "x.rs", 1, 2, "fn empty()", &[])
            .unwrap();
        let same = encode_blob(&svc.embed_text("same code chunk"));
        db.insert_code_chunk("same-a", &svc.bank_key, "a.rs", 1, 2, "fn same_a()", &same)
            .unwrap();
        db.insert_code_chunk("same-b", &svc.bank_key, "b.rs", 1, 2, "fn same_b()", &same)
            .unwrap();
        let hits = svc.search_code("same code chunk", 0, 2.0).unwrap();
        assert!(hits.is_empty() || hits.len() <= 1);
        let tied = svc.search_code("same code chunk", 8, -1.0).unwrap();
        assert!(tied.len() >= 2);

        assert!(chunk_source("", 40, 10).is_empty());
        let windows = chunk_source(&long, 40, 10);
        assert!(windows.len() > 1);
        let _ = perm;
    }

    #[test]
    fn index_skips_chunks_shorter_than_forty_chars() {
        let dir = tempdir().unwrap();
        let data = dir.path().join("data");
        let project = dir.path().join("proj");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(project.join("src/blank.rs"), "\n".repeat(50)).unwrap();
        std::fs::write(
            project.join("src/lib.rs"),
            "/// Long enough helper for indexing.\npub fn remember_index_probe() {}\n",
        )
        .unwrap();
        let svc = MemoryService::open(&project, &data, MemorySettings::default()).unwrap();
        let n = svc.index_codebase(10, 10).unwrap();
        assert!(n >= 1);
    }
}
