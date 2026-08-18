use super::*;

fn store() -> IndexStore {
    let mut s = IndexStore::new();
    s.insert(0, "src".into(), true, 0);
    s.insert(0, "src/main.rs".into(), false, 100);
    s.insert(0, "src/lib.rs".into(), false, 50);
    s.insert(0, "docs".into(), true, 0);
    s.insert(0, "docs/guide.md".into(), false, 20);
    s.insert(0, "README.md".into(), false, 10);
    s.insert(1, "ext.txt".into(), false, 5);
    s
}

#[test]
fn insert_dedups_and_updates() {
    let mut s = store();
    let n = s.len();
    assert!(!s.insert(0, "src/main.rs".into(), false, 200)); // update
    assert_eq!(s.len(), n);
    assert!(s.insert(0, "new.rs".into(), false, 1)); // insert
    assert_eq!(s.len(), n + 1);
    let e = s
        .entries()
        .iter()
        .find(|e| &*e.rel == "src/main.rs")
        .unwrap();
    assert_eq!(e.size, 200);
}

#[test]
fn remove_keeps_map_consistent() {
    let mut s = store();
    assert!(s.remove(0, "src/main.rs"));
    assert!(!s.remove(0, "src/main.rs")); // already gone
    assert!(!s.remove(0, "nope.rs"));
    // Everything else still findable (swap_remove bookkeeping).
    assert!(s.remove(0, "src/lib.rs"));
    assert!(s.remove(0, "docs/guide.md"));
    assert!(s.remove(1, "ext.txt"));
    assert_eq!(s.len(), 3);
}

#[test]
fn remove_tree_drops_descendants() {
    let mut s = store();
    let removed = s.remove_tree(0, "src");
    assert_eq!(removed, 3); // src + two files
    assert_eq!(s.len(), 4);
    assert!(!s.entries().iter().any(|e| e.rel.starts_with("src")));
}

#[test]
fn browse_is_depth_one_dirs_first() {
    let s = store();
    let top: Vec<&str> = s.browse(0, "").iter().map(|e| &*e.rel).collect();
    assert_eq!(top, vec!["docs", "src", "README.md"]); // dirs first, alpha
    let src: Vec<&str> = s.browse(0, "src").iter().map(|e| &*e.rel).collect();
    assert_eq!(src, vec!["src/lib.rs", "src/main.rs"]);
    let ext = s.browse(1, "");
    assert_eq!(ext.len(), 1);
    assert!(
        s.browse(0, "docs")
            .iter()
            .all(|e| &*e.rel == "docs/guide.md")
    );
}

#[test]
fn empty_store_and_iter_root() {
    let s = IndexStore::new();
    assert!(s.is_empty());
    assert_eq!(s.iter_root(0).count(), 0);
    let s = store();
    assert!(!s.is_empty());
    assert_eq!(s.iter_root(1).count(), 1);
    let mut s = store();
    s.clear();
    assert!(s.is_empty());
}
