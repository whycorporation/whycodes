//! Lightweight codebase RAG (hash embeddings over source chunks).

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::embed::{cosine, decode_blob, encode_blob};
use crate::error::{MemoryError, Result};
use crate::service::{CodeHit, FTS_CANDIDATES, MemoryService, SearchPage};

const EXT_OK: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "py", "go", "java", "kt", "c", "h", "cpp", "hpp", "cs", "rb",
    "php", "swift", "md", "toml", "yaml", "yml", "json", "sql", "sh", "fish", "css", "html", "vue",
    "svelte",
];

/// Files a [`MemoryService::refresh_code_index`] changed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct IndexRefresh {
    /// New files indexed.
    pub added: usize,
    /// Files modified since they were indexed, indexed again.
    pub updated: usize,
    /// Indexed files that no longer exist (or are no longer walked).
    pub removed: usize,
}

/// Whether `path` was modified after `indexed_at` (RFC 3339). A file that
/// cannot be checked counts as changed: indexing it again drops it if gone.
fn changed_since(path: &Path, indexed_at: &str) -> bool {
    let Ok(at) = chrono::DateTime::parse_from_rfc3339(indexed_at) else {
        return true;
    };
    let modified = std::fs::metadata(path).and_then(|m| m.modified());
    !modified.is_ok_and(|m| chrono::DateTime::<chrono::Utc>::from(m) <= at)
}

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
            n += self.index_file(&db, &root, &rel, max_chunks - n)?;
        }
        Ok(n)
    }

    /// Bring an existing index up to date without rebuilding it: drop files
    /// that are gone, re-index files modified since they were indexed, then
    /// add new files while there is room under `max_chunks`. An index built
    /// once and never refreshed recalls code (and line ranges) that no
    /// longer exist.
    pub fn refresh_code_index(&self, max_files: usize, max_chunks: usize) -> Result<IndexRefresh> {
        if !self.settings.enabled {
            return Err(MemoryError::msg("memory is disabled"));
        }
        let root = crate::project_key::project_root(&self.project_path);
        let mut files = Vec::new();
        walk_files(&root, &mut files, max_files);
        files.sort();
        let walked: HashSet<&str> = files.iter().map(String::as_str).collect();

        let db = self.open_db()?;
        let mut out = IndexRefresh::default();
        let mut total = 0usize;
        let mut known = HashSet::new();
        let mut stale = Vec::new();
        for (path, chunks, indexed_at) in db.code_chunk_files(&self.bank_key)? {
            if !walked.contains(path.as_str()) {
                db.delete_code_chunks_for_path(&self.bank_key, &path)?;
                out.removed += 1;
            } else if changed_since(&root.join(&path), &indexed_at) {
                db.delete_code_chunks_for_path(&self.bank_key, &path)?;
                stale.push(path.clone());
            } else {
                total += chunks;
            }
            known.insert(path);
        }
        let fresh = files.iter().filter(|rel| !known.contains(*rel));
        for (rel, new) in stale
            .iter()
            .map(|rel| (rel, false))
            .chain(fresh.map(|rel| (rel, true)))
        {
            if total >= max_chunks {
                break;
            }
            let added = self.index_file(&db, &root, rel, max_chunks - total)?;
            total += added;
            match (new, added > 0) {
                (false, _) => out.updated += 1,
                (true, true) => out.added += 1,
                (true, false) => {}
            }
        }
        Ok(out)
    }

    /// Index up to `budget` chunks of one file (chunks under 40 chars are
    /// skipped). Unreadable and huge files add nothing.
    fn index_file(
        &self,
        db: &whycodes_storage::db::Database,
        root: &Path,
        rel: &str,
        budget: usize,
    ) -> Result<usize> {
        let Some(content) = read_indexable(&root.join(rel)) else {
            return Ok(0);
        };
        let mut n = 0usize;
        for (start, end, text) in chunk_source(&content, 40, 10) {
            if n >= budget {
                break;
            }
            let text = text.trim();
            if text.len() < 40 {
                continue;
            }
            // Prefix path so search matches file-oriented queries
            let indexed = format!("// {rel}:{start}-{end}\n{text}");
            let blob = encode_blob(&self.embed_text(&indexed));
            let id = uuid::Uuid::new_v4().to_string();
            put_chunk(db, &id, &self.bank_key, rel, start, end, text, &blob)?;
            n += 1;
        }
        Ok(n)
    }

    /// Index again the files behind `rows` that changed after they were
    /// indexed, so recall does not quote code (or line ranges) that an edit
    /// earlier in the session replaced; a deleted file just drops out. Only
    /// candidate files are checked, one `stat` each. The file keeps all its
    /// chunks: an edit that grows it must not lose the new code to the cap.
    fn reindex_stale(
        &self,
        db: &whycodes_storage::db::Database,
        rows: &[whycodes_storage::models::CodeChunkRow],
    ) -> Result<bool> {
        let root = crate::project_key::project_root(&self.project_path);
        let mut seen = HashSet::new();
        let mut any = false;
        for row in rows {
            if !seen.insert(row.path.as_str())
                || !changed_since(&root.join(&row.path), &row.updated_at)
            {
                continue;
            }
            db.delete_code_chunks_for_path(&self.bank_key, &row.path)?;
            self.index_file(db, &root, &row.path, usize::MAX)?;
            any = true;
        }
        Ok(any)
    }

    /// Semantic search over indexed code chunks.
    pub fn search_code(&self, query: &str, top_k: usize, min_score: f32) -> Result<Vec<CodeHit>> {
        Ok(self.search_code_page(query, top_k, min_score)?.hits)
    }

    /// [`Self::search_code`] with match / bank counts for a card header.
    pub fn search_code_page(
        &self,
        query: &str,
        top_k: usize,
        min_score: f32,
    ) -> Result<SearchPage<CodeHit>> {
        let db = self.open_db()?;
        let mut page = db.search_code_chunks(&self.bank_key, query, FTS_CANDIDATES)?;
        if self.reindex_stale(&db, &page.rows)? {
            page = db.search_code_chunks(&self.bank_key, query, FTS_CANDIDATES)?;
        }
        let q = self.embed_text(query);
        let relevance = page.relevance;
        let mut hits: Vec<(CodeHit, f64)> = page
            .rows
            .into_iter()
            .enumerate()
            .filter_map(|(i, entry)| {
                let v = decode_blob(&entry.embedding);
                if v.is_empty() {
                    return None;
                }
                let score = cosine(&q, &v);
                let rank = path_ranked(&relevance, i, &entry.path, query);
                (score >= min_score).then_some((CodeHit { entry, score }, rank))
            })
            .collect();
        // BM25 ranks code better than the hash cosine, which common words
        // ("where", "how", "file") dominate, and a query word in the path
        // (`compact` → `compact.rs`) is strong evidence BM25 cannot see: the
        // index holds chunk text only. The cosine still sets the floor and the
        // shown score; it orders only an unranked page (a browse or a scan).
        let key = |h: &(CodeHit, f64)| if page.ranked { h.1 } else { h.0.score as f64 };
        hits.sort_by(|a, b| {
            key(b)
                .partial_cmp(&key(a))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut hits: Vec<CodeHit> = hits.into_iter().map(|(hit, _)| hit).collect();
        hits.truncate(top_k.max(1));
        Ok(SearchPage {
            hits,
            matched: page.matched,
            corpus: page.corpus,
        })
    }
}

/// What one query word found in a chunk's path adds to its rank, where the
/// best BM25 candidate counts 1.0. On 22 questions about this repo, chunks on
/// topic went 28% (cosine order) → 47% (BM25) → 57% with this weight, and on
/// 12 held-out questions 35% → 58% (questions with a relevant chunk 9 → 11).
/// Taking path matches first instead (any weight high enough) lets generic
/// path words (`turn`, `command`, `load`) push out the only relevant chunk.
const PATH_WORD_WEIGHT: f64 = 0.2;

/// Rank of candidate `i` on a BM25 page: its relevance scaled so the best
/// candidate is 1.0, plus [`PATH_WORD_WEIGHT`] for each query word of three
/// or more characters that is also a word of `path`. 0.0 off a ranked page.
fn path_ranked(relevance: &[f64], i: usize, path: &str, query: &str) -> f64 {
    let (Some(best), Some(own)) = (relevance.first(), relevance.get(i)) else {
        return 0.0;
    };
    let words = |text: &str| -> std::collections::HashSet<String> {
        text.split(|c: char| !c.is_alphanumeric())
            .map(str::to_lowercase)
            .collect()
    };
    let path_words = words(path);
    let in_path = words(query)
        .iter()
        .filter(|w| w.chars().count() >= 3 && path_words.contains(*w))
        .count();
    own / best.max(f64::MIN_POSITIVE) + PATH_WORD_WEIGHT * in_path as f64
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

/// Largest file the code index reads.
const MAX_INDEXED_FILE_BYTES: u64 = 400_000;

/// The file's text when it is UTF-8 and at most [`MAX_INDEXED_FILE_BYTES`].
/// Reads no more than one byte past the cap: a multi-GB `dump.sql` used to
/// be read whole before the size check, at every auto-index.
fn read_indexable(path: &Path) -> Option<String> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut text = String::new();
    file.take(MAX_INDEXED_FILE_BYTES + 1)
        .read_to_string(&mut text)
        .ok()?;
    (text.len() as u64 <= MAX_INDEXED_FILE_BYTES).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::MemorySettings;
    use tempfile::tempdir;

    #[test]
    fn ranked_hits_keep_bm25_order_and_a_browse_sorts_by_score() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let svc = MemoryService::open(&project, dir.path().join("data"), MemorySettings::default())
            .unwrap();
        let db = svc.open_db().unwrap();
        // The files exist, unchanged since indexing: search keeps their rows.
        for path in ["a.rs", "b.rs", "c.rs"] {
            std::fs::write(project.join(path), "").unwrap();
        }
        let query = "retry backoff";
        // Stored vectors disagree with the text on purpose: the chunk BM25
        // ranks first carries an unrelated vector, the weak match the query's
        // own, so a cosine re-rank would reverse the order.
        let chunks = [
            (
                "a.rs",
                "fn retry_backoff() { retry(); backoff(); } // retry backoff",
                "zz qq",
            ),
            (
                "b.rs",
                "fn other_thing() {} // see retry elsewhere in a long comment",
                query,
            ),
            ("c.rs", "fn unrelated_parser() {}", "parser"),
        ];
        for (i, (path, text, vector_of)) in chunks.iter().enumerate() {
            let blob = encode_blob(&svc.embed_text(vector_of));
            db.insert_code_chunk(&format!("c{i}"), &svc.bank_key, path, 1, 2, text, &blob)
                .unwrap();
        }

        let bm25: Vec<String> = db
            .search_code_chunks(&svc.bank_key, query, 8)
            .unwrap()
            .rows
            .into_iter()
            .map(|r| r.path)
            .collect();
        let hits = svc.search_code(query, 8, -1.0).unwrap();
        let order: Vec<&str> = hits.iter().map(|h| h.entry.path.as_str()).collect();
        assert_eq!(order, bm25, "the cosine does not re-rank FTS candidates");

        // No indexable word: an unranked browse, ordered by score.
        let browse = svc.search_code("?", 8, -1.0).unwrap();
        assert_eq!(browse.len(), 3);
        assert!(browse.windows(2).all(|w| w[0].score >= w[1].score));
    }

    #[test]
    fn a_query_word_in_the_path_lifts_a_close_candidate() {
        let relevance = [4.0, 3.6];
        let query = "compact the conversation";
        let best = path_ranked(&relevance, 0, "crates/tui/src/app.rs", query);
        let named = path_ranked(&relevance, 1, "crates/agent/src/compact.rs", query);
        assert!((best - 1.0).abs() < 1e-9, "{best}");
        assert!((named - (0.9 + PATH_WORD_WEIGHT)).abs() < 1e-9, "{named}");
        assert!(named > best);
        // Words under three characters do not count; nothing off a ranked page.
        assert!((path_ranked(&relevance, 1, "src/to/fn.rs", "fn to") - 0.9).abs() < 1e-9);
        assert_eq!(path_ranked(&[], 0, "compact.rs", query), 0.0);
        assert_eq!(path_ranked(&relevance, 5, "compact.rs", query), 0.0);
    }

    fn set_mtime(path: &std::path::Path, at: std::time::SystemTime) {
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(at).unwrap();
    }

    /// Modified well after anything indexed so far, whatever the clock grain.
    fn touch_later(path: &std::path::Path) {
        set_mtime(
            path,
            std::time::SystemTime::now() + std::time::Duration::from_secs(120),
        );
    }

    fn indexed_paths(svc: &MemoryService) -> Vec<String> {
        let db = svc.open_db().unwrap();
        let files = db.code_chunk_files(&svc.bank_key).unwrap();
        files.into_iter().map(|(path, _, _)| path).collect()
    }

    #[test]
    fn refresh_follows_edits_new_and_deleted_files() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("proj");
        std::fs::create_dir_all(project.join("src")).unwrap();
        let src = project.join("src");
        std::fs::write(
            src.join("a.rs"),
            "pub fn original_alpha_name() -> u32 { 1 }\n",
        )
        .unwrap();
        std::fs::write(
            src.join("b.rs"),
            "pub fn bravo_goes_away_soon() -> u32 { 2 }\n",
        )
        .unwrap();
        let svc = MemoryService::open(&project, dir.path().join("data"), MemorySettings::default())
            .unwrap();
        assert_eq!(svc.index_codebase(100, 100).unwrap(), 2);
        assert_eq!(
            svc.refresh_code_index(100, 100).unwrap(),
            IndexRefresh::default()
        );

        std::fs::write(
            src.join("a.rs"),
            "pub fn renamed_alpha_entry() -> u32 { 11 }\n",
        )
        .unwrap();
        touch_later(&src.join("a.rs"));
        std::fs::remove_file(src.join("b.rs")).unwrap();
        std::fs::write(
            src.join("c.rs"),
            "pub fn charlie_is_brand_new() -> u32 { 3 }\n",
        )
        .unwrap();
        std::fs::write(src.join("d.rs"), "fn d() {}\n").unwrap(); // too short to index
        let refresh = svc.refresh_code_index(100, 100).unwrap();
        assert_eq!(
            refresh,
            IndexRefresh {
                added: 1,
                updated: 1,
                removed: 1
            }
        );
        assert_eq!(indexed_paths(&svc), ["src/a.rs", "src/c.rs"]);
        let hits = svc.search_code("renamed_alpha_entry", 4, -1.0).unwrap();
        assert!(
            hits[0].entry.text.contains("renamed_alpha_entry"),
            "{hits:?}"
        );
        let texts = svc
            .open_db()
            .unwrap()
            .list_code_chunks(&svc.bank_key, 100)
            .unwrap();
        assert!(
            !texts.iter().any(|c| c.text.contains("original_alpha_name")),
            "stale chunk kept"
        );

        // No room under the cap: a new file waits for a later refresh.
        // (`a.rs` goes back to the past, or its future mtime keeps it stale.)
        set_mtime(&src.join("a.rs"), std::time::SystemTime::UNIX_EPOCH);
        std::fs::write(
            src.join("e.rs"),
            "pub fn echo_waits_for_room() -> u32 { 5 }\n",
        )
        .unwrap();
        assert_eq!(
            svc.refresh_code_index(100, 2).unwrap(),
            IndexRefresh::default()
        );
        assert_eq!(indexed_paths(&svc).len(), 2);

        let disabled = MemoryService::open(
            &project,
            dir.path().join("data"),
            MemorySettings {
                enabled: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(disabled.refresh_code_index(100, 100).is_err());
    }

    #[test]
    fn search_reindexes_candidate_files_edited_since_indexing() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("proj");
        let src = project.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("a.rs"),
            "pub fn retry_handler() -> u32 { old_retry_body() }\n",
        )
        .unwrap();
        std::fs::write(
            src.join("b.rs"),
            "pub fn retry_handler_copy() -> u32 { gone_retry_body() }\n",
        )
        .unwrap();
        let svc = MemoryService::open(&project, dir.path().join("data"), MemorySettings::default())
            .unwrap();
        assert_eq!(svc.index_codebase(100, 100).unwrap(), 2);

        // Edited mid-session (a new line on top shifts the range) and deleted.
        std::fs::write(
            src.join("a.rs"),
            "// header line\npub fn retry_handler() -> u32 { new_retry_body() }\n",
        )
        .unwrap();
        touch_later(&src.join("a.rs"));
        std::fs::remove_file(src.join("b.rs")).unwrap();

        let hits = svc.search_code("retry_handler", 4, -1.0).unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].entry.text.contains("new_retry_body"), "{hits:?}");
        assert_eq!(hits[0].entry.end_line, 2);
        assert_eq!(indexed_paths(&svc), ["src/a.rs"]);
    }

    #[test]
    fn changed_since_counts_unreadable_state_as_changed() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("x.rs");
        std::fs::write(&file, "x").unwrap();
        let later = (chrono::Utc::now() + chrono::Duration::seconds(120)).to_rfc3339();
        assert!(!changed_since(&file, &later));
        touch_later(&file);
        assert!(changed_since(&file, &chrono::Utc::now().to_rfc3339()));
        assert!(changed_since(&file, "not a timestamp"));
        assert!(changed_since(&dir.path().join("gone.rs"), &later));
    }

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
        // A query word must occur in the chunk; `fn` is in every indexed one.
        let ranked = svc.search_code("fn item_1", 8, -1.0).unwrap();
        assert!(!ranked.is_empty());
        assert_eq!(svc.index_codebase(0, 10).unwrap(), 0);
        let _ = svc.index_codebase(1, 500);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o644)).unwrap();
        }

        // Files behind the rows below exist and predate them: search keeps them.
        for path in ["a.rs", "b.rs", "x.rs"] {
            std::fs::write(project.join(path), "").unwrap();
        }
        let db = svc.open_db().unwrap();
        db.insert_code_chunk(
            "empty-emb",
            &svc.bank_key,
            "x.rs",
            1,
            2,
            "fn same_empty()",
            &[],
        )
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

    #[test]
    fn read_indexable_stops_one_byte_past_the_cap() {
        let dir = tempdir().unwrap();
        let at_cap = dir.path().join("a.rs");
        std::fs::write(&at_cap, "a".repeat(MAX_INDEXED_FILE_BYTES as usize)).unwrap();
        assert!(read_indexable(&at_cap).is_some());
        let over = dir.path().join("b.rs");
        std::fs::write(&over, "a".repeat(MAX_INDEXED_FILE_BYTES as usize + 1)).unwrap();
        assert!(read_indexable(&over).is_none());
        assert!(read_indexable(&dir.path().join("missing.rs")).is_none());
    }
}
