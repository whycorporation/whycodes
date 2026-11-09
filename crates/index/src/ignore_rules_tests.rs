use super::*;
use std::fs;

fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().unwrap();
    for (rel, body) in files {
        let path = dir.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }
    dir
}

#[test]
fn rules_files_are_the_two_per_directory_names() {
    assert!(is_rules_file(".gitignore"));
    assert!(is_rules_file(".ignore"));
    assert!(!is_rules_file("gitignore"));
    assert!(!is_rules_file(".git"));
}

#[test]
fn parent_reads_the_directory_part() {
    assert_eq!(parent("a/b/c"), "a/b");
    assert_eq!(parent("a"), "");
}

#[test]
fn ignored_directory_hides_everything_below_it() {
    let dir = tree(&[(".gitignore", "out/\n")]);
    let mut rules = IgnoreRules::default();
    assert!(rules.is_ignored(dir.path(), "out", true));
    assert!(rules.is_ignored(dir.path(), "out/a/b.txt", false));
    assert!(!rules.is_ignored(dir.path(), "src/main.rs", false));
    // A dir-only pattern does not match a file of that name.
    assert!(!rules.is_ignored(dir.path(), "out", false));
}

#[test]
fn scratch_dir_ignores_its_own_files() {
    let dir = tree(&[(".whycodes/scratch/.gitignore", "*\n")]);
    let mut rules = IgnoreRules::default();
    assert!(rules.is_ignored(dir.path(), ".whycodes/scratch/bg-1.log", false));
    assert!(!rules.is_ignored(dir.path(), ".whycodes/rules/a.md", false));
}

#[test]
fn deeper_rules_win_and_can_re_include() {
    let dir = tree(&[(".gitignore", "*.tmp\n"), ("sub/.gitignore", "!keep.tmp\n")]);
    let mut rules = IgnoreRules::default();
    assert!(rules.is_ignored(dir.path(), "x.tmp", false));
    assert!(rules.is_ignored(dir.path(), "sub/x.tmp", false));
    assert!(!rules.is_ignored(dir.path(), "sub/keep.tmp", false));
}

#[test]
fn an_ignored_parent_beats_a_re_include_below_it() {
    let dir = tree(&[(".gitignore", "out/\n"), ("out/.gitignore", "!x\n")]);
    let mut rules = IgnoreRules::default();
    assert!(rules.is_ignored(dir.path(), "out/x", false));
}

#[test]
fn dot_ignore_outranks_gitignore_in_the_same_directory() {
    let dir = tree(&[(".gitignore", "*.md\n"), (".ignore", "!README.md\n")]);
    let mut rules = IgnoreRules::default();
    assert!(!rules.is_ignored(dir.path(), "README.md", false));
    assert!(rules.is_ignored(dir.path(), "NOTES.md", false));
}

#[test]
fn repository_exclude_file_applies_from_the_root() {
    let dir = tree(&[(".git/info/exclude", "/secret/\n")]);
    let mut rules = IgnoreRules::default();
    assert!(rules.is_ignored(dir.path(), "secret/a.txt", false));
    assert!(!rules.is_ignored(dir.path(), "src/secret/a.txt", false));
}

#[test]
fn clear_rereads_changed_rule_files() {
    let dir = tree(&[(".gitignore", "*.log\n")]);
    let mut rules = IgnoreRules::default();
    assert!(rules.is_ignored(dir.path(), "a.log", false));
    fs::remove_file(dir.path().join(".gitignore")).unwrap();
    assert!(
        rules.is_ignored(dir.path(), "a.log", false),
        "loaded rules are cached"
    );
    rules.clear();
    assert!(!rules.is_ignored(dir.path(), "a.log", false));
}

#[test]
fn bad_lines_are_skipped_and_the_rest_still_apply() {
    let dir = tree(&[(".gitignore", "a[\n*.log\n")]);
    let mut rules = IgnoreRules::default();
    assert!(rules.is_ignored(dir.path(), "x.log", false));
    assert!(!rules.is_ignored(dir.path(), "x.rs", false));
}

#[test]
fn partial_errors_are_logged_not_raised() {
    log_partial(
        Path::new("x"),
        Some(ignore::Error::Glob {
            glob: Some("a[".into()),
            err: "unclosed".into(),
        }),
    );
    log_partial(Path::new("x"), None);
}

#[test]
fn directory_cache_starts_over_past_its_bound() {
    let dir = tree(&[]);
    let mut rules = IgnoreRules::default();
    for i in 0..MAX_RULE_DIRS {
        rules.dir_rules(dir.path(), &format!("d{i}"));
    }
    assert_eq!(rules.dirs.len(), MAX_RULE_DIRS);
    rules.dir_rules(dir.path(), "d0");
    assert_eq!(
        rules.dirs.len(),
        MAX_RULE_DIRS,
        "a cached dir does not evict"
    );
    rules.dir_rules(dir.path(), "one-more");
    assert_eq!(rules.dirs.len(), 1);
}
