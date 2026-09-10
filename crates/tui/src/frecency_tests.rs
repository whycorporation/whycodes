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

#[test]
fn load_roundtrip_sets_path_and_age_buckets() {
    let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("WHYCODES_HOME");
    unsafe { std::env::set_var("WHYCODES_HOME", home.path()) };
    let project = home.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let mut f = Frecency::load(&project);
    f.record("src/lib.rs");
    let loaded = Frecency::load(&project);
    assert!(loaded.boost("src/lib.rs") > 0);

    let now = now_epoch();
    let mut aged = Frecency::ephemeral();
    aged.inner.map.insert(
        "day-old.rs".into(),
        Stats {
            count: 1,
            last: now - 10_000,
        },
    );
    aged.inner.map.insert(
        "week-old.rs".into(),
        Stats {
            count: 1,
            last: now - 200_000,
        },
    );
    aged.inner.map.insert(
        "ancient.rs".into(),
        Stats {
            count: 1,
            last: now - 8 * 86_400,
        },
    );
    assert_eq!(aged.boost("day-old.rs"), 20 + 8);
    assert_eq!(aged.boost("week-old.rs"), 8 + 8);
    assert_eq!(aged.boost("ancient.rs"), 2 + 8);

    match prev {
        Some(v) => unsafe { std::env::set_var("WHYCODES_HOME", v) },
        None => unsafe { std::env::remove_var("WHYCODES_HOME") },
    }
}

#[test]
fn save_skips_when_parent_is_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("blocked");
    std::fs::write(&blocked, b"not-a-dir").unwrap();
    let mut f = Frecency::ephemeral();
    f.inner.path = Some(blocked.join("x.json"));
    f.record("a.rs");
}
