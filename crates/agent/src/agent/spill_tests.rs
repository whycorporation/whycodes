use super::*;

#[test]
fn store_writes_once_and_points_at_the_project_relative_file() {
    let dir = tempfile::tempdir().unwrap();
    let pointer = store_artifact(dir.path(), "toolu_01AB/cd.ef", &"x".repeat(5_000)).unwrap();
    assert_eq!(pointer, ".whycodes/scratch/artifacts/toolu_01AB_cd_ef.txt");
    let path = dir
        .path()
        .join(".whycodes/scratch/artifacts/toolu_01AB_cd_ef.txt");
    assert_eq!(std::fs::read_to_string(&path).unwrap().len(), 5_000);

    // A second body under the same id does not overwrite the first.
    assert!(store_artifact(dir.path(), "toolu_01AB/cd.ef", "later").is_some());
    assert_eq!(std::fs::read_to_string(&path).unwrap().len(), 5_000);

    assert_eq!(safe_file_stem(""), "tool");
    assert_eq!(safe_file_stem(&"a".repeat(100)).len(), 80);

    let mut store = artifact_store(dir.path().to_path_buf());
    assert!(store("call_0", "body").unwrap().ends_with("call_0.txt"));
}

#[test]
fn store_declines_when_the_scratch_dir_cannot_be_created_or_written() {
    let dir = tempfile::tempdir().unwrap();
    // `.whycodes` is a file: the artifact dir cannot be created.
    std::fs::write(dir.path().join(".whycodes"), "not a dir").unwrap();
    assert!(store_artifact(dir.path(), "t1", "body").is_none());

    // The artifact path is a directory: the write fails.
    let dir2 = tempfile::tempdir().unwrap();
    let blocked = dir2.path().join(".whycodes/scratch/artifacts/t2.txt");
    std::fs::create_dir_all(&blocked).unwrap();
    assert!(store_artifact(dir2.path(), "t2", "body").is_none());
}
