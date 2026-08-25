use super::*;

#[test]
fn change_mapping_filters_policy() {
    let roots = vec![PathBuf::from("/proj")];
    let mut out = Vec::new();
    push_change(
        &roots,
        Path::new("/proj/src/main.rs"),
        ChangeKind::Upsert,
        &mut out,
    );
    push_change(
        &roots,
        Path::new("/proj/target/x.o"),
        ChangeKind::Upsert,
        &mut out,
    );
    push_change(
        &roots,
        Path::new("/proj/.env"),
        ChangeKind::Upsert,
        &mut out,
    );
    push_change(
        &roots,
        Path::new("/other/f.rs"),
        ChangeKind::Upsert,
        &mut out,
    );
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].rel, "src/main.rs");
    assert_eq!(out[0].root, 0);
}

#[test]
fn map_event_covers_create_remove_modify_rename() {
    use notify::event::{CreateKind, ModifyKind, RemoveKind, RenameMode};
    let roots = vec![PathBuf::from("/proj")];
    let ev = |kind, paths: Vec<PathBuf>| Event {
        kind,
        paths,
        attrs: Default::default(),
    };
    let created = map_event(
        &roots,
        &ev(
            EventKind::Create(CreateKind::File),
            vec![PathBuf::from("/proj/src/a.rs")],
        ),
    );
    assert_eq!(created[0].kind, ChangeKind::Upsert);
    let removed = map_event(
        &roots,
        &ev(
            EventKind::Remove(RemoveKind::File),
            vec![PathBuf::from("/proj/src/a.rs")],
        ),
    );
    assert_eq!(removed[0].kind, ChangeKind::Remove);
    let renamed = map_event(
        &roots,
        &ev(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            vec![
                PathBuf::from("/proj/src/old.rs"),
                PathBuf::from("/proj/src/new.rs"),
            ],
        ),
    );
    assert_eq!(renamed[0].kind, ChangeKind::Remove);
    assert_eq!(renamed[1].kind, ChangeKind::Upsert);
    let single = map_event(
        &roots,
        &ev(
            EventKind::Modify(ModifyKind::Name(RenameMode::To)),
            vec![PathBuf::from("/proj/src/solo.rs")],
        ),
    );
    assert_eq!(single[0].kind, ChangeKind::Upsert);
    let data = map_event(
        &roots,
        &ev(
            EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Any)),
            vec![PathBuf::from("/proj/src/a.rs")],
        ),
    );
    assert_eq!(data[0].kind, ChangeKind::Upsert);
    let access = map_event(
        &roots,
        &ev(EventKind::Access(notify::event::AccessKind::Any), vec![]),
    );
    assert!(access.is_empty());
    let folder = map_event(
        &roots,
        &ev(
            EventKind::Create(CreateKind::Folder),
            vec![PathBuf::from("/proj/src")],
        ),
    );
    assert_eq!(folder[0].kind, ChangeKind::Upsert);
    let any_rm = map_event(
        &roots,
        &ev(
            EventKind::Remove(RemoveKind::Any),
            vec![PathBuf::from("/proj/src/a.rs")],
        ),
    );
    assert_eq!(any_rm[0].kind, ChangeKind::Remove);
    let root_self = map_event(
        &roots,
        &ev(
            EventKind::Create(CreateKind::Any),
            vec![PathBuf::from("/proj")],
        ),
    );
    assert!(root_self.is_empty());
}

#[test]
fn spawn_watches_a_temp_root() {
    let dir = tempfile::TempDir::new().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let w = spawn(&[dir.path().to_path_buf()], tx);
    assert!(w.is_some(), "watcher should install on a temp dir");
    std::fs::write(dir.path().join("f.rs"), "x").unwrap();
    let _ = rx.recv_timeout(std::time::Duration::from_secs(2));
    drop(w);
}

#[test]
fn spawn_missing_root_returns_none() {
    let (tx, _rx) = std::sync::mpsc::channel();
    let w = spawn(&[PathBuf::from("/no/such/whycodes-index-root")], tx);
    assert!(w.is_none());
    log_watcher_unavailable(&"boom");
}
