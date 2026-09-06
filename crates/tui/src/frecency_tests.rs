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
