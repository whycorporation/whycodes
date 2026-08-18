use super::*;

fn engine() -> FuzzyEngine {
    let e = FuzzyEngine::default();
    let inj = e.injector();
    FuzzyEngine::push(&inj, "src/main.rs", false);
    FuzzyEngine::push(&inj, "src/lib.rs", false);
    FuzzyEngine::push(&inj, "crates/tui/src/app.rs", false);
    FuzzyEngine::push(&inj, "docs", true);
    FuzzyEngine::push(&inj, "README.md", false);
    drop(inj);
    e
}

#[test]
fn query_finds_paths_with_indices() {
    let mut e = engine();
    let hits = e.query_blocking("main.rs", 10);
    assert!(!hits.is_empty());
    assert_eq!(hits[0].rel, "src/main.rs");
    assert!(!hits[0].is_dir);
    assert!(!hits[0].indices.is_empty());
    assert!(hits[0].score >= min_score("main.rs"));
}

#[test]
fn query_matches_subsequence_across_dirs() {
    let mut e = engine();
    let hits = e.query_blocking("tuiapp", 10);
    assert!(
        hits.iter().any(|h| h.rel == "crates/tui/src/app.rs"),
        "{hits:?}"
    );
}

#[test]
fn short_query_drops_weak_tail() {
    let mut e = engine();
    let hits = e.query_blocking("zzzzz", 10);
    assert!(hits.is_empty(), "{hits:?}");
    let inj = e.injector();
    for i in 0..40 {
        FuzzyEngine::push(&inj, &format!("pad/file{i}.txt"), false);
    }
    drop(inj);
    let _ = e.query_blocking("e", 40);
    let _ = e.query_blocking("f", 40);
}

#[test]
fn dirs_strip_trailing_slash() {
    let mut e = engine();
    let hits = e.query_blocking("docs", 10);
    assert!(hits.iter().any(|h| h.rel == "docs" && h.is_dir));
}

#[test]
fn restart_clears_items() {
    let mut e = engine();
    assert!(!e.query_blocking("main", 10).is_empty());
    e.restart();
    assert!(e.query_blocking("main", 10).is_empty());
}

#[test]
fn empty_query_is_browse_not_fuzzy() {
    let mut e = engine();
    assert!(e.query_blocking("", 10).is_empty());
}

#[test]
fn score_filters() {
    assert!(below_floor(1, 10));
    assert!(!below_floor(20, 10));
}

#[test]
fn matcher_thread_count_scales() {
    assert_eq!(matcher_thread_count(1), 2);
    assert_eq!(matcher_thread_count(4), 2);
    assert_eq!(matcher_thread_count(5), 3);
    assert_eq!(matcher_thread_count(8), 3);
    assert_eq!(matcher_thread_count(16), 4);
}

#[test]
fn set_query_incremental_and_nudge() {
    let mut e = engine();
    e.set_query("ma");
    e.set_query("ma");
    e.set_query("main");
    let _ = e.nudge();
    let (hits, _) = e.read(10);
    assert!(hits.iter().any(|h| h.rel.ends_with("main.rs")), "{hits:?}");
    e.set_query("main ");
    let _ = e.read(5);
}
