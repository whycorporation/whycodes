use super::*;
use std::path::Path;

#[test]
fn language_from_extension() {
    assert_eq!(language_for_path(Path::new("a.rs")), Some(Lang::Rust));
    assert_eq!(
        language_for_path(Path::new("a.TSX")),
        Some(Lang::TypeScript)
    );
    assert_eq!(language_for_path(Path::new("a.js")), Some(Lang::TypeScript));
    assert_eq!(
        language_for_path(Path::new("a.mjs")),
        Some(Lang::TypeScript)
    );
    assert_eq!(
        language_for_path(Path::new("a.cjs")),
        Some(Lang::TypeScript)
    );
    assert_eq!(
        language_for_path(Path::new("a.jsx")),
        Some(Lang::TypeScript)
    );
    assert_eq!(language_for_path(Path::new("a.py")), Some(Lang::Python));
    assert_eq!(language_for_path(Path::new("a.md")), None);
    assert_eq!(language_for_path(Path::new("Makefile")), None);
}

#[test]
fn rust_extracts_named_fn() {
    let src = "fn foo() { let x = 1; }\nfn bar() { if x { 1 } else { 0 } }\n";
    let fns = rust_functions(src);
    assert_eq!(fns.len(), 2);
    assert_eq!(fns[0].name, "foo");
    assert_eq!(fns[1].name, "bar");
    assert!(fns[0].sloc >= 1);
}

#[test]
fn rust_skips_forward_decl_and_ident_fn() {
    let src = "fn proto();\nlet fn_name = 1;\nfn real() { 1 }\n";
    let fns = rust_functions(src);
    assert_eq!(
        fns.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        ["real"]
    );
}

#[test]
fn rust_anonymous_and_async_ident() {
    let src = "fn () { 1 }\nfn async_fn() { 2 }\n";
    let fns = rust_functions(src);
    assert!(fns.iter().any(|f| f.name == "fn"));
    assert!(fns.iter().any(|f| f.name == "async_fn"));
}

#[test]
fn functions_dispatch() {
    assert!(!functions(Lang::Rust, "fn a() { 1 }").is_empty());
    assert!(!functions(Lang::TypeScript, "function a() { return 1; }").is_empty());
    assert!(!functions(Lang::Python, "def a():\n    return 1\n").is_empty());
}

#[test]
fn ts_and_py_extract() {
    let ts = "function handle(x) { if (x) { return 1; } else { return 0; } }\n";
    let fns = ts_functions(ts);
    assert_eq!(fns.len(), 1);
    assert_eq!(fns[0].name, "handle");
    let py = "def foo():\n    if True:\n        return 1\n    return 0\n\ndef bar():\n    pass\n";
    let fns = py_functions(py);
    assert_eq!(fns.len(), 2);
    assert_eq!(fns[0].name, "foo");
    assert_eq!(fns[1].name, "bar");
    let empty_def = py_functions("def ():\n    pass\n");
    assert_eq!(empty_def[0].name, "fn");
}

#[test]
fn sloc_skips_comments() {
    assert_eq!(sloc("a\n\n// c\n# p\nb\n"), 2);
}

#[test]
fn brace_body_skips_strings_and_line_comments() {
    let src = "fn x() { let s = \"{\"; // }\nok\n}";
    let (body, end) = brace_body(src, 0).unwrap();
    assert!(body.contains("ok"));
    assert_eq!(end, src.len());
    assert!(brace_body("fn x();", 0).is_none());
    assert!(brace_body("fn x() ", 0).is_none());
    assert!(brace_body("fn x() { let n = 1;", 0).is_none());
    let long = format!("fn x({}) {{ 1 }}", "a".repeat(500));
    assert!(brace_body(&long, 4).is_none());
}

#[test]
fn skip_string_handles_escape_and_unclosed() {
    let s = br#""a\"b""#;
    assert_eq!(skip_string(s, 0), s.len());
    let open = b"\"abc";
    assert_eq!(skip_string(open, 0), open.len());
    assert_eq!(strip_async_kw("async foo"), "foo");
    assert_eq!(strip_async_kw("async_fn"), "async_fn");
    assert_eq!(strip_async_kw("bar"), "bar");
}

#[test]
fn match_fn_name_rejects_embedded() {
    assert!(match_fn_name("xfn y", 1, "fn").is_none());
    assert!(match_fn_name("fnx()", 0, "fn").is_none());
    assert_eq!(match_fn_name("fn foo", 0, "fn").as_deref(), Some("foo"));
    assert!(match_fn_name("notfn", 0, "fn").is_none());
    assert_eq!(match_fn_name("fn", 0, "fn").as_deref(), Some("fn"));
}

#[test]
fn brace_body_skips_non_string_chars() {
    let src = "fn x() { let n = 1; }";
    let (body, _) = brace_body(src, 0).unwrap();
    assert!(body.contains("let n"));
}

#[test]
fn py_blank_lines_inside_def_and_comment_sloc() {
    let src = "def foo():\n\n    return 1\nnot_a_fn = 1\n";
    let fns = py_functions(src);
    assert_eq!(fns.len(), 1);
    assert!(fns[0].body.contains("return 1"));
    assert_eq!(sloc(""), 0);
}
