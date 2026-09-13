use super::*;
use std::path::Path;

#[test]
fn thresholds_default_and_from_parts() {
    let t = Thresholds::default();
    assert_eq!(t.verbosity, 0.25);
    assert_eq!(t.erosion, 0.50);
    assert_eq!(t.delta_loc, 800);
    let t = Thresholds::from_parts(0.1, 0.2, 10, 0.3, 0.4, 20, 0);
    assert_eq!(t.hotspots, 1);
    assert_eq!(t.block_delta_loc, 20);
}

#[test]
fn verdict_ok_review_block() {
    let t = Thresholds::default();
    assert_eq!(verdict_of(0.1, 0.1, 10, &t), Verdict::Ok);
    assert_eq!(verdict_of(0.26, 0.1, 10, &t), Verdict::Review);
    assert_eq!(verdict_of(0.1, 0.51, 10, &t), Verdict::Review);
    assert_eq!(verdict_of(0.1, 0.1, 801, &t), Verdict::Review);
    assert_eq!(verdict_of(0.41, 0.1, 10, &t), Verdict::Block);
    assert_eq!(verdict_of(0.1, 0.76, 10, &t), Verdict::Block);
    assert_eq!(verdict_of(0.1, 0.1, 5000, &t), Verdict::Block);
    assert_eq!(Verdict::Ok.as_str(), "ok");
    assert_eq!(Verdict::Review.to_string(), "review");
    assert_eq!(Verdict::Block.as_str(), "block");
}

#[test]
fn analyze_files_stable_on_fixtures() {
    let clean = SourceFile {
        path: "src/add.rs".into(),
        content: "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n".into(),
    };
    let r = analyze_files(
        &[clean],
        12,
        1,
        "abc",
        "working-tree",
        &Thresholds::default(),
    );
    assert_eq!(r.verdict, Verdict::Ok);
    assert_eq!(r.base, "abc");
    assert_eq!(r.head, "working-tree");
    assert_eq!(r.files_changed, 1);
    assert_eq!(r.delta_loc, 12);
    assert!(r.verbosity < 0.25);
    assert!(r.erosion < 0.50);
    assert!(r.unparsed_files.is_empty());

    let mut body = String::from("fn handle(x: i32) -> i32 {\n");
    for i in 0..16 {
        body.push_str(&format!(
            "    if x == {i} || x == {} {{\n        repeated_helper_call();\n    }}\n",
            i + 50
        ));
    }
    body.push_str("    repeated_helper_call();\n    repeated_helper_call();\n}\n");
    let sloppy = SourceFile {
        path: "src/handle.rs".into(),
        content: body,
    };
    let r = analyze_files(
        &[sloppy],
        200,
        1,
        "HEAD",
        "working-tree",
        &Thresholds::default(),
    );
    assert!(r.verbosity >= 0.15, "verbosity {}", r.verbosity);
    assert!(r.erosion >= 0.50, "erosion {}", r.erosion);
    assert_ne!(r.verdict, Verdict::Ok);
    assert_eq!(r.hotspots[0].function, "handle");
    assert!(r.hotspots[0].cc > 10);
}

#[test]
fn format_report_includes_hotspots_and_unparsed() {
    let r = Report {
        base: "abc".into(),
        head: "working-tree".into(),
        delta_loc: 4,
        files_changed: 1,
        verbosity: 0.1,
        erosion: 0.2,
        hotspots: vec![Hotspot {
            path: "a.rs".into(),
            function: "f".into(),
            cc: 12,
            sloc: 40,
        }],
        verdict: Verdict::Ok,
        thresholds: ThresholdsOut {
            verbosity: 0.25,
            erosion: 0.5,
            delta_loc: 800,
        },
        unparsed_files: vec!["notes.md".into()],
    };
    let s = format_report(&r);
    assert!(s.contains("Slop"));
    assert!(s.contains("a.rs"));
    assert!(s.contains("notes.md"));
    assert!(s.contains("CC=12"));
    let empty = Report {
        hotspots: vec![],
        unparsed_files: vec![],
        ..r
    };
    let s = format_report(&empty);
    assert!(s.contains("(none)"));
}

#[test]
fn analyze_git_tree() {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap()
    };
    assert!(git(&["init", "-b", "main"]).status.success());
    let _ = git(&["config", "user.email", "t@t"]);
    let _ = git(&["config", "user.name", "t"]);
    std::fs::write(dir.path().join("a.rs"), "fn a() { 1 }\n").unwrap();
    assert!(git(&["add", "."]).status.success());
    assert!(git(&["commit", "-m", "init"]).status.success());
    std::fs::write(
        dir.path().join("a.rs"),
        "fn a() { if true { 1 } else { 0 } }\n",
    )
    .unwrap();
    let r = analyze(dir.path(), Some("HEAD"), &Thresholds::default()).unwrap();
    assert_eq!(r.head, "working-tree");
    assert_eq!(r.files_changed, 1);
    assert_eq!(r.verdict, Verdict::Ok);
    let err = analyze(Path::new("/no/such/repo-xyz"), None, &Thresholds::default()).unwrap_err();
    assert!(!err.to_string().is_empty());
}

#[test]
fn language_for_path_reexport() {
    assert_eq!(language_for_path(Path::new("x.py")), Some(Lang::Python));
}

#[test]
fn error_display() {
    let e = Error {
        message: "boom".into(),
    };
    assert_eq!(e.to_string(), "boom");
}
