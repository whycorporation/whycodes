use super::*;

#[test]
fn prunes_heavy_dirs() {
    for d in ["target", "node_modules", ".git", "dist", ".venv"] {
        assert!(is_pruned_dir(d), "{d} should be pruned");
    }
    for d in ["src", "crates", "docs", "tests"] {
        assert!(!is_pruned_dir(d), "{d} should be kept");
    }
}

#[test]
fn hidden_rules() {
    assert!(!is_pruned_dir(".github"));
    assert!(!is_pruned_dir(".whycodes"));
    assert!(is_pruned_dir(".idea"));
    assert!(!is_pruned_file(".gitignore"));
    assert!(is_pruned_file(".env"));
    assert!(!is_pruned_file("main.rs"));
}

#[test]
fn rel_path_policy() {
    assert!(rel_path_allowed("src/main.rs"));
    assert!(rel_path_allowed(".github/workflows/ci.yml"));
    assert!(!rel_path_allowed("target/debug/build.o"));
    assert!(!rel_path_allowed("node_modules/pkg/index.js"));
    assert!(!rel_path_allowed(".env"));
    assert!(!rel_path_allowed("nested/.cache/x"));
    assert!(is_pruned_dir(""));
    assert!(is_pruned_dir("."));
    assert!(is_pruned_dir(".."));
    assert!(rel_path_allowed(""));
}
