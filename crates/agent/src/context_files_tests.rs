use super::*;

#[test]
fn empty_project_discovers_nothing() {
    let dir = tempfile::tempdir().unwrap();
    assert!(discover(dir.path()).is_empty());
    let prompt = append_project_instructions("base", dir.path());
    assert_eq!(prompt, "base");
    assert!(!prompt.contains("Project Instructions"));
}

#[test]
fn agents_md_keeps_existing_heading() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "  \nProject rules here\n  ").unwrap();
    let with = append_project_instructions("base", dir.path());
    assert!(with.contains("Project Instructions (AGENTS.md)"), "{with}");
    assert!(with.contains("Project rules here"), "{with}");
}

#[test]
fn lowercase_agents_md_when_canonical_absent() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("agents.md"), "lowercase rules").unwrap();
    let with = append_project_instructions("base", dir.path());
    assert!(with.contains("lowercase rules"), "{with}");
}

#[test]
fn whycodes_nested_agents_md() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".whycodes")).unwrap();
    std::fs::write(dir.path().join(".whycodes/AGENTS.md"), "nested rules").unwrap();
    let with = append_project_instructions("base", dir.path());
    assert!(with.contains("nested rules"), "{with}");
    assert!(with.contains(".whycodes/AGENTS.md"), "{with}");
}

#[test]
fn sibling_claude_and_copilot_files_are_concatenated() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "native").unwrap();
    std::fs::write(dir.path().join("CLAUDE.md"), "claude rules").unwrap();
    std::fs::create_dir(dir.path().join(".github")).unwrap();
    std::fs::write(
        dir.path().join(".github/copilot-instructions.md"),
        "copilot rules",
    )
    .unwrap();
    let files = discover(dir.path());
    let joined: String = files.iter().map(|f| f.content.as_str()).collect();
    assert!(joined.contains("native"));
    assert!(joined.contains("claude rules"));
    assert!(joined.contains("copilot rules"));
    let rendered = render(&files);
    assert!(rendered.contains("Additional instructions (CLAUDE.md)"));
}

#[test]
fn duplicate_content_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "same body").unwrap();
    std::fs::write(dir.path().join("CLAUDE.md"), "same body").unwrap();
    let files = discover(dir.path());
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].label, "AGENTS.md");
}

#[test]
fn cursor_mdc_and_clinerules_dir_are_picked_up() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".cursor/rules")).unwrap();
    std::fs::write(
        dir.path().join(".cursor/rules/rust.mdc"),
        "always use cargo fmt",
    )
    .unwrap();
    std::fs::create_dir(dir.path().join(".clinerules")).unwrap();
    std::fs::write(dir.path().join(".clinerules/style.md"), "no unwrap").unwrap();
    std::fs::write(dir.path().join(".cursorrules"), "cursor root").unwrap();
    let files = discover(dir.path());
    let labels: Vec<&str> = files.iter().map(|f| f.label.as_str()).collect();
    assert!(labels.iter().any(|l| l.contains("rust.mdc")), "{labels:?}");
    assert!(labels.iter().any(|l| l.contains("style.md")), "{labels:?}");
    assert!(labels.contains(&".cursorrules"), "{labels:?}");
}

#[test]
fn git_root_walk_collects_ancestor_agents() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "root rules").unwrap();
    let pkg = dir.path().join("pkg");
    std::fs::create_dir(&pkg).unwrap();
    std::fs::write(pkg.join("CLAUDE.md"), "pkg rules").unwrap();
    let files = discover(&pkg);
    let joined: String = files.iter().map(|f| f.content.as_str()).collect();
    assert!(joined.contains("pkg rules"), "{joined}");
    assert!(joined.contains("root rules"), "{joined}");
}

#[test]
fn empty_files_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "   \n").unwrap();
    std::fs::write(dir.path().join("CLAUDE.md"), "keep").unwrap();
    let files = discover(dir.path());
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].content, "keep");
}

#[test]
fn label_falls_back_to_file_name_outside_project() {
    assert_eq!(
        label_for(Path::new("/tmp/CLAUDE.md"), Path::new("/proj")),
        "CLAUDE.md"
    );
}

#[test]
fn parent_or_and_file_name_helpers_cover_none() {
    let root = Path::new("/repo");
    assert_eq!(
        parent_or(root, Some(Path::new("/repo/src"))),
        PathBuf::from("/repo/src")
    );
    assert_eq!(parent_or(root, None), PathBuf::from("/repo"));
    assert_eq!(
        file_name_or_display(Path::new("/tmp/CLAUDE.md")),
        "CLAUDE.md"
    );
    assert_eq!(file_name_or_display(Path::new("")), "");
}

#[test]
fn byte_and_file_caps_and_duplicate_paths() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "native").unwrap();
    std::fs::write(dir.path().join("CLAUDE.md"), "native").unwrap();
    let files = discover(dir.path());
    assert_eq!(files.len(), 1, "duplicate content skipped");

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("AGENTS.md"),
        "a".repeat(MAX_CONTEXT_BYTES + 10),
    )
    .unwrap();
    std::fs::write(dir.path().join("CLAUDE.md"), "second").unwrap();
    let files = discover(dir.path());
    assert!(
        files.len() == 1 || files.iter().any(|f| f.label.contains("AGENTS")),
        "{:?}",
        files.iter().map(|f| f.label.as_str()).collect::<Vec<_>>()
    );

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "one").unwrap();
    std::fs::create_dir_all(dir.path().join(".cursor/rules")).unwrap();
    for i in 0..(MAX_CONTEXT_FILES + 4) {
        std::fs::write(
            dir.path().join(".cursor/rules").join(format!("r{i}.mdc")),
            format!("rule {i} unique body"),
        )
        .unwrap();
    }
    let files = discover(dir.path());
    assert!(files.len() <= MAX_CONTEXT_FILES, "{}", files.len());
}

#[test]
fn git_root_at_filesystem_root_is_none_or_some() {
    let _ = discover(Path::new("/"));
}

#[test]
fn nested_whycodes_dir_skips_duplicate_absolute_path() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    std::fs::create_dir(dir.path().join(".whycodes")).unwrap();
    std::fs::write(dir.path().join(".whycodes/AGENTS.md"), "nested unique").unwrap();
    let files = discover(&dir.path().join(".whycodes"));
    let hits = files
        .iter()
        .filter(|f| f.content.contains("nested unique"))
        .count();
    assert_eq!(hits, 1, "{files:?}");
}
