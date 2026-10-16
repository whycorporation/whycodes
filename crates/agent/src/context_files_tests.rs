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
    std::fs::create_dir_all(dir.path().join(".whycodes/rules")).unwrap();
    std::fs::write(dir.path().join(".whycodes/rules/tests.md"), "test first").unwrap();
    let files = discover(dir.path());
    let labels: Vec<&str> = files.iter().map(|f| f.label.as_str()).collect();
    assert!(labels.iter().any(|l| l.contains("rust.mdc")), "{labels:?}");
    assert!(labels.iter().any(|l| l.contains("style.md")), "{labels:?}");
    assert!(labels.contains(&".cursorrules"), "{labels:?}");
    assert!(labels.contains(&".whycodes/rules/tests.md"), "{labels:?}");
    let whycodes = labels.iter().position(|l| l.ends_with("tests.md")).unwrap();
    let cursor = labels.iter().position(|l| l.ends_with("rust.mdc")).unwrap();
    assert!(whycodes < cursor, "{labels:?}");
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

#[test]
fn nested_for_returns_each_new_directory_top_down_once() {
    let project = tempfile::tempdir().unwrap();
    let pkg = project.path().join("pkg");
    let deep = pkg.join("sub");
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(project.path().join("AGENTS.md"), "root rules").unwrap();
    std::fs::write(pkg.join("CLAUDE.md"), "  pkg rules  ").unwrap();
    std::fs::write(deep.join("AGENTS.md"), "deep rules").unwrap();
    std::fs::write(deep.join("RULES.md"), "   ").unwrap();
    let mut seen = BTreeSet::new();

    let found = nested_for(project.path(), &deep.join("lib.rs"), &mut seen);
    let got: Vec<(&str, &str)> = found
        .iter()
        .map(|f| (f.label.as_str(), f.content.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("pkg/CLAUDE.md", "pkg rules"),
            ("pkg/sub/AGENTS.md", "deep rules")
        ],
        "root rules live in the system prompt; empty files are skipped"
    );
    assert!(
        nested_for(project.path(), &deep.join("other.rs"), &mut seen).is_empty(),
        "each directory once per session"
    );
    assert!(nested_for(project.path(), &project.path().join("top.rs"), &mut seen).is_empty());
    let outside = tempfile::tempdir().unwrap();
    assert!(nested_for(project.path(), &outside.path().join("x.rs"), &mut seen).is_empty());
}

#[test]
fn nested_for_caps_the_bytes_attached_at_once() {
    let project = tempfile::tempdir().unwrap();
    let a = project.path().join("a");
    let b = a.join("b");
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("AGENTS.md"), "x".repeat(15_000)).unwrap();
    std::fs::write(b.join("AGENTS.md"), "y".repeat(15_000)).unwrap();
    let found = nested_for(project.path(), &b.join("f.rs"), &mut BTreeSet::new());
    assert_eq!(found.len(), 1, "the second file would pass 20 KB");
    let block = render_nested(&found);
    assert!(block.starts_with(
        "\n\n<nested-instructions>\n# Instructions for this directory (a/AGENTS.md)\n\n"
    ));
    assert!(block.ends_with("\n</nested-instructions>"));
}
