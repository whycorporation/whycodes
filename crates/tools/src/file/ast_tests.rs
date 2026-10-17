use super::*;
use ast_grep_core::tree_sitter::LanguageExt;
use serde_json::json;
use std::fs;
use tempfile::TempDir;

fn scope(pattern: &str, language: Option<AstLang>, root: &Path) -> Scope {
    Scope {
        pattern: pattern.into(),
        language,
        root: root.to_path_buf(),
        working_dir: root.to_string_lossy().into_owned(),
    }
}

fn target(dir: &TempDir, name: &str, body: &str, lang: AstLang) -> Target {
    let path = dir.path().join(name);
    fs::write(&path, body).unwrap();
    Target {
        path,
        shown: name.into(),
        lang,
    }
}

#[test]
fn language_names_and_extensions() {
    for (name, lang) in [
        ("Rust", AstLang::Rust),
        ("rs", AstLang::Rust),
        ("py", AstLang::Python),
        ("python", AstLang::Python),
        ("golang", AstLang::Go),
        ("go", AstLang::Go),
        ("js", AstLang::JavaScript),
        ("jsx", AstLang::JavaScript),
        ("mjs", AstLang::JavaScript),
        ("cjs", AstLang::JavaScript),
        ("javascript", AstLang::JavaScript),
        (" typescript ", AstLang::TypeScript),
        ("ts", AstLang::TypeScript),
        ("mts", AstLang::TypeScript),
        ("cts", AstLang::TypeScript),
        ("tsx", AstLang::Tsx),
    ] {
        assert_eq!(AstLang::from_name(name), Some(lang), "{name}");
    }
    assert_eq!(AstLang::from_name("cobol"), None);
    assert_eq!(
        AstLang::from_file(Path::new("a/b.PYI")),
        Some(AstLang::Python)
    );
    assert_eq!(AstLang::from_file(Path::new("x.tsx")), Some(AstLang::Tsx));
    assert_eq!(AstLang::from_file(Path::new("x.rs")), Some(AstLang::Rust));
    assert_eq!(AstLang::from_file(Path::new("x.golang")), None);
    assert_eq!(AstLang::from_file(Path::new("x.txt")), None);
    assert_eq!(AstLang::from_file(Path::new("Makefile")), None);
    let names: Vec<&str> = ALL_LANGS.iter().map(|l| l.name()).collect();
    assert_eq!(names.join(", "), LANGUAGE_NAMES);
}

#[test]
fn typescript_filter_covers_tsx() {
    assert!(AstLang::Tsx.selected_by(AstLang::TypeScript));
    assert!(AstLang::TypeScript.selected_by(AstLang::TypeScript));
    assert!(!AstLang::TypeScript.selected_by(AstLang::Tsx));
    assert!(!AstLang::JavaScript.selected_by(AstLang::TypeScript));
}

#[test]
fn sigils_swap_only_for_metavariables() {
    assert_eq!(swap_sigils("$A + $_ + $$$B", 'µ'), "µA + µ_ + µµµB");
    assert_eq!(swap_sigils("f($$$)", 'µ'), "f(µµµ)");
    assert_eq!(swap_sigils("x$$$", 'µ'), "xµµµ");
    assert_eq!(swap_sigils("\"$a\" $", 'µ'), "\"$a\" $");
    assert_eq!(
        AstLang::Rust.pre_process_pattern("$A.unwrap()"),
        "µA.unwrap()"
    );
    assert!(matches!(
        AstLang::JavaScript.pre_process_pattern("$A"),
        Cow::Borrowed("$A")
    ));
    assert_eq!(AstLang::Go.expando_char(), 'µ');
    assert_eq!(AstLang::Tsx.expando_char(), '$');
}

#[test]
fn every_language_parses_and_matches() {
    let cases = [
        (
            AstLang::Rust,
            "fn main() { a.unwrap(); }",
            "$X.unwrap()",
            "a",
        ),
        (AstLang::Python, "print(a)\n", "print($X)", "a"),
        (AstLang::Go, "package m\nfunc f() { g(a) }\n", "g($X)", "a"),
        (AstLang::JavaScript, "foo(a);", "foo($X)", "a"),
        (
            AstLang::TypeScript,
            "let v: number = a;",
            "let $V: number = $X",
            "a",
        ),
        (
            AstLang::Tsx,
            "const e = <div>{a}</div>;",
            "<div>{$X}</div>",
            "a",
        ),
    ];
    for (lang, src, pat, want) in cases {
        let p = Pattern::try_new(pat, lang).unwrap();
        assert!(!p.has_error(), "{lang:?}");
        let root = lang.ast_grep(src);
        let m = root.root().find(&p).expect("match");
        assert_eq!(m.get_env().get_match("X").unwrap().text(), want);
        assert!(lang.kind_to_id("identifier") > 0, "{lang:?}");
    }
    assert!(AstLang::Rust.field_to_id("name").is_some());
    assert!(AstLang::Rust.field_to_id("no_such_field").is_none());
}

#[test]
fn scope_args_validate() {
    let wd = "/w";
    let missing = scope_from_args(&json!({}), wd).err().unwrap();
    assert!(missing.contains("pattern"));
    let blank = scope_from_args(&json!({"pattern": "  "}), wd)
        .err()
        .unwrap();
    assert!(blank.contains("pattern"));
    let err = scope_from_args(&json!({"pattern": "x", "language": "cobol"}), wd)
        .err()
        .unwrap();
    assert!(err.contains("unknown language `cobol`") && err.contains(LANGUAGE_NAMES));
    let s = scope_from_args(&json!({"pattern": "x", "language": " ", "path": "src"}), wd).unwrap();
    assert_eq!(s.language, None);
    assert_eq!(s.root, Path::new("/w").join("src"));
    let s = scope_from_args(&json!({"pattern": "x", "language": "py"}), wd).unwrap();
    assert_eq!(s.language, Some(AstLang::Python));
    assert_eq!(s.root, Path::new("/w"));
}

#[test]
fn targets_from_file_and_dir() {
    let dir = TempDir::new().unwrap();
    let missing = scope("x", None, &dir.path().join("nope"));
    let err = collect_targets(&missing).err().unwrap();
    assert!(err.contains("path not found"));

    fs::write(dir.path().join("notes.txt"), "x").unwrap();
    let txt = scope("x", None, &dir.path().join("notes.txt"));
    let err = collect_targets(&txt).err().unwrap();
    assert!(err.contains("pass `language`"));
    let forced = scope("x", Some(AstLang::Python), &dir.path().join("notes.txt"));
    let t = collect_targets(&forced).unwrap();
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].lang, AstLang::Python);

    fs::write(dir.path().join("a.rs"), "fn a() {}").unwrap();
    let single = scope("x", None, &dir.path().join("a.rs"));
    assert_eq!(collect_targets(&single).unwrap()[0].lang, AstLang::Rust);

    fs::create_dir(dir.path().join("web")).unwrap();
    fs::write(dir.path().join("web/b.ts"), "let b = 1;").unwrap();
    fs::write(dir.path().join("web/c.tsx"), "let c = 1;").unwrap();
    fs::write(dir.path().join("d.py"), "d = 1\n").unwrap();
    let all = collect_targets(&scope("x", None, dir.path())).unwrap();
    let shown: Vec<&str> = all.iter().map(|t| t.shown.as_str()).collect();
    assert_eq!(shown.len(), 4, "{shown:?}");
    assert!(shown.windows(2).all(|w| w[0] <= w[1]));
    let ts = collect_targets(&scope("x", Some(AstLang::TypeScript), dir.path())).unwrap();
    let mut langs: Vec<AstLang> = ts.iter().map(|t| t.lang).collect();
    langs.sort();
    assert_eq!(langs, vec![AstLang::TypeScript, AstLang::Tsx]);
}

#[test]
fn patterns_compile_per_language() {
    let dir = TempDir::new().unwrap();
    let rs = target(&dir, "a.rs", "", AstLang::Rust);
    let py = target(&dir, "b.py", "", AstLang::Python);
    let rs2 = target(&dir, "c.rs", "", AstLang::Rust);
    let targets = vec![rs, py, rs2];

    // A Rust item is not valid Python: Python is skipped, Rust kept.
    let s = scope("$A.unwrap()?", None, dir.path());
    let ok = compile_patterns(&s, &targets).unwrap();
    let langs: Vec<AstLang> = ok.iter().map(|(l, _)| *l).collect();
    assert_eq!(langs, vec![AstLang::Rust]);
    assert!(pattern_for(&ok, AstLang::Rust).is_some());
    assert!(pattern_for(&ok, AstLang::Python).is_none());

    // Explicit language keeps a pattern even with error nodes.
    let forced = scope("$A.unwrap()?", Some(AstLang::Python), dir.path());
    assert_eq!(compile_patterns(&forced, &targets[1..2]).unwrap().len(), 1);

    // Nothing accepts it: one error naming each language.
    let bad = scope("a; b;", None, dir.path());
    let err = compile_patterns(&bad, &targets).err().unwrap();
    assert!(err.contains("rust:") && err.contains("python:"), "{err}");

    let multi = scope("$$$A", Some(AstLang::Rust), dir.path());
    assert!(compile_patterns(&multi, &targets[..1]).is_err());

    // No targets: no patterns and no error.
    assert!(compile_patterns(&s, &[]).unwrap().is_empty());
}

#[test]
fn read_source_skips_large_and_binary() {
    let dir = TempDir::new().unwrap();
    let big = dir.path().join("big.rs");
    fs::write(&big, vec![b'a'; MAX_GREP_FILE_BYTES as usize + 1]).unwrap();
    assert!(read_source(&big).is_none());
    let bin = dir.path().join("bin.rs");
    fs::write(&bin, [0xff, 0xfe, 0x00]).unwrap();
    assert!(read_source(&bin).is_none());
    let ok = dir.path().join("ok.rs");
    fs::write(&ok, "fn a() {}").unwrap();
    assert_eq!(read_source(&ok).as_deref(), Some("fn a() {}"));
}

#[test]
fn captures_single_multi_and_empty() {
    let src = "foo(a, b + c);\nfoo();\nbar(x, y);";
    let root = AstLang::JavaScript.ast_grep(src);
    let p = Pattern::try_new("foo($$$ARGS)", AstLang::JavaScript).unwrap();
    let hits: Vec<_> = root.root().find_all(&p).collect();
    assert_eq!(captures(&hits[0], src), vec!["$$$ARGS = a, b + c"]);
    assert_eq!(captures(&hits[1], src), vec!["$$$ARGS = "]);
    let p = Pattern::try_new("bar($A, $_)", AstLang::JavaScript).unwrap();
    let m = root.root().find(&p).unwrap();
    assert_eq!(captures(&m, src), vec!["$A = x"]);
    for var in [MetaVariable::Dropped(false), MetaVariable::Multiple] {
        assert_eq!(describe_capture(var, m.get_env(), src), None);
    }
}

#[test]
fn one_line_clips() {
    assert_eq!(one_line(""), "");
    assert_eq!(one_line("abc"), "abc");
    assert_eq!(one_line("a\nb"), "a…");
    let long = "x".repeat(250);
    assert_eq!(one_line(&long).chars().count(), 201);
}

fn rep(start: usize, end: usize, text: &str) -> Replace {
    Replace {
        start,
        end,
        text: text.into(),
    }
}

#[test]
fn replacements_apply_in_order() {
    let edits = [rep(0, 1, "X"), rep(2, 3, "YY")];
    assert_eq!(apply_replacements("a b c", &edits), "X YY c");
    assert_eq!(apply_replacements("abc", &[]), "abc");
}

#[test]
fn line_helpers() {
    assert_eq!(line_starts(""), vec![0]);
    assert_eq!(line_starts("a\nb\n"), vec![0, 2]);
    assert_eq!(line_starts("a\nb"), vec![0, 2]);
    let starts = line_starts("a\nb\nc");
    assert_eq!(line_of(&starts, 0), 0);
    assert_eq!(line_of(&starts, 2), 1);
    assert_eq!(line_of(&starts, 4), 2);
    assert_eq!(line_strs("a\r\nb\n"), vec!["a", "b"]);
    assert_eq!(hunk_start(4, 0), 4);
    assert_eq!(hunk_start(4, 2), 5);
}

#[test]
fn diff_single_change_with_context() {
    let src = "1\n2\n3\n4\nfoo\n6\n7\n8\n9\n";
    let at = src.find("foo").unwrap();
    let (d, added, removed) = unified_diff("f.rs", src, &[rep(at, at + 3, "bar")]);
    assert_eq!(
        d,
        "--- a/f.rs\n+++ b/f.rs\n@@ -2,7 +2,7 @@\n 2\n 3\n 4\n-foo\n+bar\n 6\n 7\n 8\n"
    );
    assert_eq!((added, removed), (1, 1));
}

#[test]
fn diff_two_edits_on_one_line_and_merged_hunks() {
    let src = "a(1); a(2);\nx\ny\nz\na(3);\n";
    let p = |s: &str| src.find(s).unwrap();
    let edits = [
        rep(p("1"), p("1") + 1, "one"),
        rep(p("2"), p("2") + 1, "two"),
        rep(p("3"), p("3") + 1, "three\nmore"),
    ];
    let (d, added, removed) = unified_diff("f.js", src, &edits);
    assert_eq!(
        d,
        "--- a/f.js\n+++ b/f.js\n@@ -1,5 +1,6 @@\n-a(1); a(2);\n+a(one); a(two);\n x\n y\n z\n-a(3);\n+a(three\n+more);\n"
    );
    assert_eq!((added, removed), (3, 2));
}

#[test]
fn diff_separate_hunks_shift_new_lines() {
    let mut src = String::from("a\n");
    for i in 0..10 {
        src.push_str(&format!("{i}\n"));
    }
    src.push_str("b\n");
    let a = src.find('a').unwrap();
    let b = src.find('b').unwrap();
    let edits = [rep(a, a + 1, "a1\na2"), rep(b, b + 1, "B")];
    let (d, _, _) = unified_diff("f", &src, &edits);
    assert!(
        d.contains("@@ -1,4 +1,5 @@\n-a\n+a1\n+a2\n 0\n 1\n 2\n"),
        "{d}"
    );
    assert!(d.contains("@@ -9,4 +10,4 @@\n 7\n 8\n 9\n-b\n+B\n"), "{d}");
}

#[test]
fn diff_deleting_whole_file_body() {
    let (d, added, removed) = unified_diff("f.py", "foo", &[rep(0, 3, "")]);
    assert_eq!(d, "--- a/f.py\n+++ b/f.py\n@@ -1,1 +0,0 @@\n-foo\n");
    assert_eq!((added, removed), (0, 1));
}

#[test]
fn described_rust_example_matches() {
    let p = Pattern::try_new("fn $F($$$ARGS) -> Result<$T> { $$$ }", AstLang::Rust).unwrap();
    assert!(!p.has_error());
    let src = "fn load(a: u8, b: u8) -> Result<String> {\n    let x = 1;\n    todo!()\n}\n";
    let root = AstLang::Rust.ast_grep(src);
    let m = root.root().find(&p).expect("match");
    assert_eq!(
        captures(&m, src),
        vec!["$$$ARGS = a: u8, b: u8", "$F = load", "$T = String"]
    );
}
