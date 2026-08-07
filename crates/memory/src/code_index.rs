//! Lightweight codebase RAG (hash embeddings over source chunks).

use std::path::Path;

use crate::embed::{cosine, decode_blob, encode_blob};
use crate::service::{CodeHit, MemoryService};

const SKIP_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "dist",
    "build",
    ".next",
    "vendor",
    "__pycache__",
    ".venv",
    "venv",
    ".whycode",
];

const EXT_OK: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "py", "go", "java", "kt", "c", "h", "cpp", "hpp", "cs", "rb",
    "php", "swift", "md", "toml", "yaml", "yml", "json", "sql", "sh", "fish", "css", "html", "vue",
    "svelte",
];

impl MemoryService {
    /// Walk the project, chunk text files, embed, replace prior index for this bank.
    pub fn index_codebase(&self, max_files: usize, max_chunks: usize) -> anyhow::Result<usize> {
        if !self.settings.enabled {
            anyhow::bail!("memory is disabled");
        }
        let root = crate::project_key::project_root(&self.project_path);
        let mut files = Vec::new();
        walk_files(&root, &root, &mut files, max_files)?;
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
                db.insert_code_chunk(
                    &id,
                    &self.bank_key,
                    &rel,
                    start as i64,
                    end as i64,
                    text,
                    &blob,
                )?;
                n += 1;
            }
        }
        Ok(n)
    }

    /// Semantic search over indexed code chunks.
    pub fn search_code(
        &self,
        query: &str,
        top_k: usize,
        min_score: f32,
    ) -> anyhow::Result<Vec<CodeHit>> {
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

fn walk_files(
    root: &Path,
    dir: &Path,
    out: &mut Vec<String>,
    max_files: usize,
) -> std::io::Result<()> {
    if out.len() >= max_files {
        return Ok(());
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    for ent in entries.flatten() {
        if out.len() >= max_files {
            break;
        }
        let path = ent.path();
        let name = ent.file_name().to_string_lossy().to_string();
        if name.starts_with('.') && name != ".github" {
            // still allow .github? skip dot dirs except none
            if path.is_dir() {
                continue;
            }
        }
        if path.is_dir() {
            if SKIP_DIRS.iter().any(|s| *s == name) {
                continue;
            }
            walk_files(root, &path, out, max_files)?;
        } else if path.is_file() {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !EXT_OK.iter().any(|e| *e == ext) {
                continue;
            }
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    Ok(())
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
}
