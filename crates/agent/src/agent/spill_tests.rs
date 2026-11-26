use super::*;

#[test]
fn store_writes_once_and_points_at_the_project_relative_file() {
    let dir = tempfile::tempdir().unwrap();
    let body = "x".repeat(5_000);
    let pointer = store_artifact(dir.path(), "toolu_01AB/cd.ef", &body).unwrap();
    let name = format!("toolu_01AB_cd_ef-{:016x}.txt", fnv1a(&body));
    assert_eq!(pointer, format!(".whycodes/scratch/artifacts/{name}"));
    let path = dir.path().join(".whycodes/scratch/artifacts").join(&name);
    assert_eq!(std::fs::read_to_string(&path).unwrap().len(), 5_000);
    assert!(dir.path().join(".whycodes/scratch/.gitignore").is_file());

    // The same body again reuses its file.
    assert_eq!(
        store_artifact(dir.path(), "toolu_01AB/cd.ef", &body).as_deref(),
        Some(pointer.as_str())
    );

    assert_eq!(safe_file_stem(""), "tool");
    assert_eq!(safe_file_stem(&"a".repeat(100)).len(), 80);
    assert_eq!(fnv1a(""), 0xcbf2_9ce4_8422_2325);

    let mut store = artifact_store(dir.path().to_path_buf());
    assert!(store("call_0", "body").unwrap().contains("call_0-"));
}

/// Regression: a repeated id (`gcall_1` in every Gemini stream) pointed the
/// later result at the earlier call's file.
#[test]
fn a_reused_id_with_a_new_body_gets_its_own_file() {
    let dir = tempfile::tempdir().unwrap();
    let first = store_artifact(dir.path(), "gcall_1", "first output").unwrap();
    let second = store_artifact(dir.path(), "gcall_1", "second output").unwrap();
    assert_ne!(first, second);
    let read = |p: &str| std::fs::read_to_string(dir.path().join(p)).unwrap();
    assert_eq!(read(&first), "first output");
    assert_eq!(read(&second), "second output");
}

#[test]
fn store_declines_when_the_scratch_dir_cannot_be_created_or_written() {
    let dir = tempfile::tempdir().unwrap();
    // `.whycodes` is a file: the artifact dir cannot be created.
    std::fs::write(dir.path().join(".whycodes"), "not a dir").unwrap();
    assert!(store_artifact(dir.path(), "t1", "body").is_none());

    // The artifact path is a directory: the write fails.
    let dir2 = tempfile::tempdir().unwrap();
    let blocked = dir2
        .path()
        .join(".whycodes/scratch/artifacts")
        .join(format!("t2-{:016x}.txt", fnv1a("body")));
    std::fs::create_dir_all(&blocked).unwrap();
    assert!(store_artifact(dir2.path(), "t2", "body").is_none());
}
