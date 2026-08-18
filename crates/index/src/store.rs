//! In-memory entry store: the source of truth for tools and browse mode.
//!
//! Entries are root-relative `/`-separated paths. A `FxHashMap` index gives
//! O(1) dedup and removal for watcher deltas; linear scans back `browse` and
//! glob filtering (50k entries ≈ single-digit ms, cheaper than any syscall
//! storm the tools used to pay per call).

use rustc_hash::FxHashMap;

/// One indexed entry.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Root-relative path with `/` separators, e.g. `crates/tui/src/app.rs`.
    pub rel: Box<str>,
    /// Which root this entry belongs to (index into `WorkspaceIndex::roots`).
    pub root: u16,
    /// True for directories.
    pub is_dir: bool,
    /// File size in bytes (0 for dirs and symlinks).
    pub size: u64,
}

/// Per-root view of the entry store.
#[derive(Debug, Default)]
pub struct IndexStore {
    entries: Vec<Entry>,
    /// `(root, rel)` → position in `entries`.
    by_key: FxHashMap<(u16, Box<str>), u32>,
}

impl IndexStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Insert or update an entry. Returns true when newly inserted.
    pub fn insert(&mut self, root: u16, rel: Box<str>, is_dir: bool, size: u64) -> bool {
        let key = (root, rel.clone());
        if let Some(&idx) = self.by_key.get(&key) {
            let e = &mut self.entries[idx as usize];
            e.is_dir = is_dir;
            e.size = size;
            return false;
        }
        let idx = self.entries.len() as u32;
        self.entries.push(Entry {
            rel,
            root,
            is_dir,
            size,
        });
        self.by_key.insert(key, idx);
        true
    }

    /// Remove one exact entry. Returns true when it existed.
    pub fn remove(&mut self, root: u16, rel: &str) -> bool {
        // Lookup allocates an owned key; watcher deltas are rare, so the
        // simple path beats raw-entry hashing here.
        let key = (root, Box::<str>::from(rel));
        let Some(idx) = self.by_key.remove(&key) else {
            return false;
        };
        let idx = idx as usize;
        self.entries.swap_remove(idx);
        if idx < self.entries.len() {
            let moved = &self.entries[idx];
            let moved_key = (moved.root, moved.rel.clone());
            self.by_key.insert(moved_key, idx as u32);
        }
        true
    }

    /// Remove an entry and every descendant (watcher dir-removal).
    pub fn remove_tree(&mut self, root: u16, rel: &str) -> usize {
        let prefix = format!("{rel}/");
        let mut removed = 0usize;
        // Collect first to keep swap_remove bookkeeping simple.
        let victims: Vec<Box<str>> = self
            .entries
            .iter()
            .filter(|e| e.root == root && (&*e.rel == rel || e.rel.starts_with(&prefix)))
            .map(|e| e.rel.clone())
            .collect();
        for rel in victims {
            if self.remove(root, &rel) {
                removed += 1;
            }
        }
        removed
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.by_key.clear();
    }

    /// Depth-1 children of `rel_dir` within `root` (`""` = top level),
    /// dirs first then case-insensitive name order.
    pub fn browse(&self, root: u16, rel_dir: &str) -> Vec<&Entry> {
        let prefix = if rel_dir.is_empty() {
            String::new()
        } else {
            format!("{}/", rel_dir.trim_end_matches('/'))
        };
        let mut out: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|e| {
                e.root == root && e.rel.starts_with(prefix.as_str()) && {
                    let rest = &e.rel[prefix.len()..];
                    !rest.is_empty() && !rest.contains('/')
                }
            })
            .collect();
        out.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.rel.to_ascii_lowercase().cmp(&b.rel.to_ascii_lowercase()))
        });
        out
    }

    /// Iterate entries of one root without cloning.
    pub fn iter_root(&self, root: u16) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(move |e| e.root == root)
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
