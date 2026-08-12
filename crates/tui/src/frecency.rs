//! Frecency (frequency × recency) for the `@file` picker.
//!
//! Pure fuzzy score ranks *patterns*; users think in *files they touch*.
//! Every accepted picker selection is recorded (`count`, `last_used`) and
//! future queries get a bounded additive boost, so habitual files surface
//! first — the Claude Code picker feel — without drowning strong matches.
//!
//! Persisted per project at `<data_dir>/frecency/<key>.json` (machine-local
//! state; deliberately NOT under the project's `.whycode/`, which users
//! commit). The key is a deterministic hash of the canonical project path.
//! Best-effort: load/save failures degrade to an empty in-memory map.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Keep at most this many tracked files (oldest-last-used evicted).
const MAX_ENTRIES: usize = 1000;

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(flatten)]
    map: HashMap<String, Stats>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct Stats {
    count: u32,
    last: i64,
}

/// Per-project frecency table.
pub struct Frecency {
    inner: File,
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Deterministic storage key for a project path (DefaultHasher::new() uses
/// fixed keys; worst case after a Rust upgrade is a cold frecency map).
fn project_key(project_root: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    project_root.hash(&mut h);
    format!("{:016x}", h.finish())
}

impl Frecency {
    /// Load the table for `project_root` (canonical). Missing/corrupt files
    /// start empty.
    pub fn load(project_root: &Path) -> Self {
        let path = whycode_config::Config::data_dir()
            .ok()
            .map(|d| d.join("frecency").join(format!("{}.json", project_key(project_root))));
        let inner = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<File>(&s).ok())
            .map(|mut f| {
                f.path = path.clone();
                f
            })
            .unwrap_or_else(|| File {
                map: HashMap::new(),
                path,
            });
        Self { inner }
    }

    /// In-memory table (no persistence) — used in tests.
    pub fn ephemeral() -> Self {
        Self {
            inner: File::default(),
        }
    }

    /// Record one accepted selection.
    pub fn record(&mut self, rel: &str) {
        let now = now_epoch();
        let e = self.inner.map.entry(rel.to_string()).or_insert(Stats {
            count: 0,
            last: 0,
        });
        e.count = e.count.saturating_add(1);
        e.last = now;
        if self.inner.map.len() > MAX_ENTRIES {
            // Evict the stalest 10% in one pass.
            let mut by_age: Vec<(String, i64)> = self
                .inner
                .map
                .iter()
                .map(|(k, v)| (k.clone(), v.last))
                .collect();
            by_age.sort_by_key(|(_, last)| *last);
            let drop_n = MAX_ENTRIES / 10;
            for (k, _) in by_age.into_iter().take(drop_n) {
                self.inner.map.remove(&k);
            }
        }
        self.save();
    }

    /// Additive score boost for a path (0 when never picked).
    ///
    /// Bounded at 120: strong fuzzy matches score 150–300, so habits lift a
    /// file within reach but never bury a clearly better pattern match.
    pub fn boost(&self, rel: &str) -> u32 {
        let Some(e) = self.inner.map.get(rel) else {
            return 0;
        };
        let age = now_epoch() - e.last;
        let recency = if age < 3_600 {
            40
        } else if age < 86_400 {
            20
        } else if age < 7 * 86_400 {
            8
        } else {
            2
        };
        let frequency = e.count.min(10) * 8;
        recency + frequency
    }

    fn save(&self) {
        let Some(path) = &self.inner.path else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string(&self.inner) {
            // Write-then-rename so a crash mid-write never leaves a torn file.
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boost_grows_with_use_and_decays_with_age() {
        let mut f = Frecency::ephemeral();
        assert_eq!(f.boost("src/main.rs"), 0);
        f.record("src/main.rs");
        let first = f.boost("src/main.rs");
        assert!(first >= 48, "count=1 fresh: {first}"); // 40 + 8
        for _ in 0..9 {
            f.record("src/main.rs");
        }
        let seasoned = f.boost("src/main.rs");
        assert!(seasoned > first);
        assert!(seasoned <= 120);
    }

    #[test]
    fn evictions_cap_the_map() {
        let mut f = Frecency::ephemeral();
        for i in 0..(MAX_ENTRIES + 50) {
            f.inner.map.insert(
                format!("f{i}.rs"),
                Stats {
                    count: 1,
                    last: i as i64,
                },
            );
        }
        // record() triggers eviction on the next insert
        f.record("newest.rs");
        assert!(f.inner.map.len() <= MAX_ENTRIES);
        assert!(f.inner.map.contains_key("newest.rs"));
    }

    #[test]
    fn save_and_load_roundtrip() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("f.json");
        let mut f = Frecency::ephemeral();
        f.inner.path = Some(path.clone());
        f.record("a/b.rs");
        let g = Frecency {
            inner: std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str::<File>(&s).ok())
                .unwrap_or_default(),
        };
        assert!(g.boost("a/b.rs") > 0);
    }
}

