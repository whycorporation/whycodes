use super::*;

#[test]
fn cyclomatic_base_and_branches() {
    assert_eq!(cyclomatic(""), 1);
    assert!(cyclomatic("if x { 1 } else if y { 2 } else { 3 }") >= 3);
    assert!(cyclomatic("match x { 1 => a, 2 => b, _ => c }") >= 3);
    assert!(cyclomatic("while x && y || z { }") >= 4);
    assert!(cyclomatic("try { } catch (e) { }") >= 2);
    assert!(cyclomatic("if x:\n    y\nelif z:\n    w\n") >= 3);
    assert!(cyclomatic("return x?") >= 2);
}

#[test]
fn clone_detects_repeated_lines() {
    let src = "    do_something_long();\n    other();\n    do_something_long();\n";
    let lines = clone_lines(src);
    assert_eq!(lines.len(), 2);
    assert!(clone_lines("short\nshort\n").is_empty());
    assert!(clone_lines("use foo::bar::baz;\nuse foo::bar::baz;\n").is_empty());
    assert!(
        clone_lines(
            "// comment long enough to ignore twice\n// comment long enough to ignore twice\n"
        )
        .is_empty()
    );
}

#[test]
fn verbosity_heuristics_each_lang() {
    let rust = verbosity_lines(
        Lang::Rust,
        "pub fn x() -> i32 { 1 }\n    Ok(())\n    todo!\n    unimplemented!\n    self.x = x;\n    v.clone()\n// === header\n    AUTO-GENERATED\n",
    );
    assert!(rust.len() >= 6, "{rust:?}");
    let ts = verbosity_lines(Lang::TypeScript, "function id(x) { return x; }\n");
    assert!(!ts.is_empty());
    let py = verbosity_lines(Lang::Python, "pass\nreturn x\n# === hdr\nTODO: generated\n");
    assert!(py.len() >= 3, "{py:?}");
}

#[test]
fn score_clean_vs_sloppy() {
    let clean = SourceFile {
        path: "clean.rs".into(),
        content: "fn add(a: i32, b: i32) -> i32 { a + b }\n".into(),
    };
    let sloppy = SourceFile {
        path: "sloppy.rs".into(),
        content: sloppy_rust(),
    };
    let c = score_files(&[clean], 8);
    let s = score_files(&[sloppy], 8);
    assert!(c.verbosity < 0.25, "clean verbosity {}", c.verbosity);
    assert!(c.erosion < 0.50, "clean erosion {}", c.erosion);
    assert!(s.verbosity >= 0.15, "sloppy verbosity {}", s.verbosity);
    assert!(s.erosion >= 0.50, "sloppy erosion {}", s.erosion);
    assert!(!s.hotspots.is_empty());
    assert_eq!(s.hotspots[0].function, "handle");
}

#[test]
fn unparsed_and_empty() {
    let other = SourceFile {
        path: "notes.md".into(),
        content: "hello".into(),
    };
    let s = score_files(&[other], 3);
    assert_eq!(s.unparsed, vec!["notes.md"]);
    assert_eq!(s.verbosity, 0.0);
    assert_eq!(s.erosion, 0.0);
    let empty = score_files(&[], 0);
    assert_eq!(empty.verbosity, 0.0);
}

fn sloppy_rust() -> String {
    let mut s = String::from("fn handle(x: i32) -> i32 {\n");
    for i in 0..20 {
        s.push_str(&format!(
            "    if x == {i} || x == {} {{\n        do_something_long();\n    }}\n",
            i + 100
        ));
    }
    s.push_str("    do_something_long();\n    do_something_long();\n}\n");
    s
}
