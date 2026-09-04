use super::*;

/// Serializes tests that read/write the process-wide syntax theme.
/// `set_syntax_theme` is global; a stream highlighter that committed
/// under Night will disagree with a batch highlight that ran after a
/// sibling test flipped the theme to Day.
fn lock_theme() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn highlights_known_languages_into_spans() {
    let lines = highlight_code_spans("let x = 1;", Some("rust"));
    assert_eq!(lines.len(), 1);
    assert!(!lines[0].is_empty());
    let text: String = lines[0].iter().map(|(_, t)| t.as_str()).collect();
    assert_eq!(text, "let x = 1;");
}

#[test]
fn an_unknown_language_still_returns_the_text() {
    for lang in [None, Some("not-a-language")] {
        let lines = highlight_code_spans("some text", lang);
        let text: String = lines[0].iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(text, "some text", "{lang:?}");
    }
}

#[test]
fn highlighting_preserves_line_count() {
    let lines = highlight_code_spans("a\nb\nc", Some("rust"));
    assert_eq!(lines.len(), 3);
}

#[test]
fn empty_lines_are_preserved() {
    let lines = highlight_code_spans("a\n\nb", Some("rust"));
    assert_eq!(lines.len(), 3);
    let mid: String = lines[1].iter().map(|(_, t)| t.as_str()).collect();
    assert_eq!(mid, "");
}

#[test]
fn multi_line_comment_stays_comment_coloured() {
    let code = "/* start\n   still comment */";
    let lines = highlight_code_spans(code, Some("rust"));
    assert_eq!(lines.len(), 2);
    let text: String = lines
        .iter()
        .map(|spans| spans.iter().map(|(_, t)| t.as_str()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(text, "/* start\n   still comment */");
}

#[test]
fn a_cached_result_matches_an_uncached_one() {
    let _theme = lock_theme();
    set_syntax_theme(SyntaxTheme::GrokNight);
    let code = "fn main() { let x = 1; }";
    let first = highlight_code_spans(code, Some("rust"));
    let second = highlight_code_spans(code, Some("rust"));
    assert_eq!(first, second);
    assert_eq!(first.as_ref(), &highlight_uncached(code, Some("rust")));
}

#[test]
fn the_language_is_part_of_the_cache_key() {
    let code = "let x = 1";
    let rust = highlight_code_spans(code, Some("rust"));
    let untagged = highlight_code_spans(code, None);
    assert_ne!(
        rust, untagged,
        "a tagged and an untagged block share text but not styling"
    );
}

#[test]
fn the_cache_stays_bounded() {
    for i in 0..CACHE_ENTRIES * 2 {
        // Fully committed (trailing newline) so each lands in closed memo.
        highlight_code_spans(&format!("let unique_{i} = {i};\n"), Some("rust"));
    }
    assert!(closed_cache().lock().unwrap().len() <= CACHE_ENTRIES);
}

#[test]
fn closed_cache_evicts_oldest_not_everything() {
    let mut cache = ClosedHighlightCache::default();
    let keep = Arc::new(vec![vec![((1, 2, 3), String::from("keep"))]]);
    cache.insert(1, Arc::clone(&keep));
    for i in 2..=CACHE_ENTRIES as u64 {
        cache.insert(i, Arc::new(vec![vec![((0, 0, 0), format!("{i}"))]]));
    }
    assert_eq!(cache.len(), CACHE_ENTRIES);
    // Recency bump so key 1 survives the next insert.
    assert!(cache.get(&1).is_some());
    cache.insert(
        CACHE_ENTRIES as u64 + 1,
        Arc::new(vec![vec![((0, 0, 0), String::from("new"))]]),
    );
    assert_eq!(cache.len(), CACHE_ENTRIES);
    assert!(cache.get(&1).is_some(), "recently used entry must survive");
    assert!(
        cache.get(&2).is_none(),
        "oldest unused entry must be evicted"
    );
}

#[test]
fn highlight_code_emits_ansi_for_rust() {
    let out = highlight_code("let x = 1;", "rust");
    assert!(out.contains("\x1b["), "expected ANSI escapes in {out:?}");
    assert!(out.contains("let"));
}

#[test]
fn highlight_code_unknown_language_is_plain() {
    assert_eq!(highlight_code("hello", "not-a-lang"), "hello");
}

#[test]
fn detect_language_from_extension() {
    assert_eq!(detect_language("src/main.rs"), Some("rust"));
    assert_eq!(detect_language("app.tsx"), Some("tsx"));
    assert_eq!(detect_language("Dockerfile"), Some("dockerfile"));
    assert_eq!(detect_language("Makefile"), Some("makefile"));
    assert_eq!(detect_language("noext"), None);
}

#[test]
fn rs_alias_resolves() {
    let lines = highlight_code_spans("fn main() {}", Some("rs"));
    let text: String = lines[0].iter().map(|(_, t)| t.as_str()).collect();
    assert_eq!(text, "fn main() {}");
    assert!(!lines[0].is_empty());
}

#[test]
fn theme_loads() {
    let t = grok_night_theme();
    let name = t.name.as_deref().unwrap_or("").to_ascii_lowercase();
    assert!(
        name.contains("grok") || t.settings.foreground.is_some(),
        "Grok Night should load, got {name:?}"
    );
}

#[test]
fn grok_night_and_day_are_distinct() {
    let night = grok_night_theme();
    let day = grok_day_theme();
    assert_ne!(
        night.settings.foreground.map(|c| (c.r, c.g, c.b)),
        day.settings.foreground.map(|c| (c.r, c.g, c.b)),
        "Grok Night and Day must not share the same default fg"
    );
}

#[test]
fn switching_syntax_theme_recolours_rust() {
    let _theme = lock_theme();
    let code = "fn main() { let x = \"hi\"; }";
    set_syntax_theme(SyntaxTheme::GrokNight);
    let night = highlight_uncached(code, Some("rust"));
    set_syntax_theme(SyntaxTheme::GrokDay);
    let day = highlight_uncached(code, Some("rust"));
    set_syntax_theme(SyntaxTheme::GrokNight);
    assert_ne!(
        night, day,
        "Grok Night vs Day must produce different token colours"
    );
}

#[test]
fn syntax_set_knows_common_languages() {
    let ps = syntax_set();
    // Present in both syntect defaults and the two-face pack.
    for token in ["rust", "python", "javascript", "go", "json"] {
        assert!(
            ps.find_syntax_by_token(token).is_some(),
            "missing syntax for {token}"
        );
    }
    #[cfg(feature = "extended-syntax")]
    for token in ["typescript", "toml"] {
        assert!(
            ps.find_syntax_by_token(token).is_some(),
            "extended pack missing syntax for {token}"
        );
    }
}

// ── Open-stream incremental ────────────────────────────────────────

#[test]
fn stream_append_matches_batch() {
    let _theme = lock_theme();
    set_syntax_theme(SyntaxTheme::GrokNight);
    let full = "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n}\n";
    let mut stream = OpenStreamHighlighter::new();
    for end in 1..=full.len() {
        if !full.is_char_boundary(end) {
            continue;
        }
        let prefix = &full[..end];
        let got = stream.highlight(prefix, Some("rust")).expect("hl");
        let batch = highlight_uncached(prefix, Some("rust"));
        assert_eq!(got, batch, "prefix len {end}: {prefix:?}");
    }
}

#[test]
fn stream_commits_only_complete_lines() {
    let mut stream = OpenStreamHighlighter::new();
    let _ = stream.highlight("let a = 1", Some("rust")).unwrap();
    assert_eq!(stream.committed_line_count(), 0, "no newline yet");

    let _ = stream.highlight("let a = 1;\nlet b", Some("rust")).unwrap();
    assert_eq!(stream.committed_line_count(), 1);

    let _ = stream
        .highlight("let a = 1;\nlet b = 2;\n", Some("rust"))
        .unwrap();
    assert_eq!(stream.committed_line_count(), 2);
    assert!(stream.is_fully_committed("let a = 1;\nlet b = 2;\n"));
}

#[test]
fn stream_language_change_rebuilds() {
    let _theme = lock_theme();
    set_syntax_theme(SyntaxTheme::GrokNight);
    let mut stream = OpenStreamHighlighter::new();
    let body = "let x = 1;\n";
    let yamlish = stream.highlight(body, Some("yaml")).unwrap();
    let rusty = stream.highlight(body, Some("rust")).unwrap();
    assert_eq!(rusty, highlight_uncached(body, Some("rust")));
    // Different grammars should not force equal colours; just ensure rust path works.
    let _ = yamlish;
}

#[test]
fn stream_non_prefix_edit_rebuilds() {
    let _theme = lock_theme();
    set_syntax_theme(SyntaxTheme::GrokNight);
    let mut stream = OpenStreamHighlighter::new();
    let _ = stream.highlight("alpha: 1\n", Some("yaml")).unwrap();
    let b = "beta: 2\n";
    let got = stream.highlight(b, Some("yaml")).unwrap();
    assert_eq!(got, highlight_uncached(b, Some("yaml")));
}

#[test]
fn with_open_code_spans_visits_committed_and_partial() {
    let _theme = lock_theme();
    set_syntax_theme(SyntaxTheme::GrokNight);
    let complete = with_open_code_spans("let a = 1;\n", Some("rust"), |committed, partial| {
        assert_eq!(committed.len(), 1);
        assert!(partial.is_none());
        committed.len()
    });
    assert_eq!(complete, Some(1));

    let open = with_open_code_spans("let a = 1;\nlet b", Some("rust"), |committed, partial| {
        assert_eq!(committed.len(), 1);
        assert!(partial.is_some());
        (
            committed.len(),
            partial.map(|p| p.iter().map(|(_, t)| t.as_str()).collect::<String>()),
        )
    });
    let (n, tail) = open.expect("rust syntax");
    assert_eq!(n, 1);
    assert_eq!(tail.as_deref(), Some("let b"));

    assert!(
        with_open_code_spans("plain", Some("not-a-language"), |_, _| 0).is_none(),
        "unknown language must skip the open-stream visitor"
    );
}

#[test]
fn public_api_stream_growth_matches_batch() {
    let _theme = lock_theme();
    set_syntax_theme(SyntaxTheme::GrokNight);
    // Exercises the global stream via highlight_code_spans (what the TUI calls).
    let full = "key: value\nlist:\n  - a\n  - b\n";
    for end in 1..=full.len() {
        if !full.is_char_boundary(end) {
            continue;
        }
        let prefix = &full[..end];
        let got = highlight_code_spans(prefix, Some("yaml"));
        let batch = highlight_uncached(prefix, Some("yaml"));
        assert_eq!(got.as_ref(), &batch, "prefix len {end}");
    }
}

#[test]
fn multi_line_comment_survives_incremental_growth() {
    let _theme = lock_theme();
    set_syntax_theme(SyntaxTheme::GrokNight);
    let full = "/* line one\n   line two */\nfn x() {}\n";
    let mut stream = OpenStreamHighlighter::new();
    // Grow line-by-line (the TUI streaming case).
    let mut acc = String::new();
    for line in full.split_inclusive('\n') {
        acc.push_str(line);
        let got = stream.highlight(&acc, Some("rust")).unwrap();
        assert_eq!(got, highlight_uncached(&acc, Some("rust")));
    }
}

#[test]
fn recover_lock_survives_poison() {
    fn poison_and_recover<T: Send + 'static>(make: fn() -> T) {
        let m = std::sync::Arc::new(Mutex::new(make()));
        let m2 = std::sync::Arc::clone(&m);
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("poison");
        })
        .join();
        let _g = recover_lock(m.lock());
    }
    poison_and_recover(ClosedHighlightCache::default);
    poison_and_recover(OpenStreamHighlighter::new);
    poison_and_recover(rustc_hash::FxHashMap::<u64, Result<Arc<Vec<String>>, String>>::default);
}

#[test]
fn public_spans_commit_partial_and_unknown() {
    let _theme = lock_theme();
    set_syntax_theme(SyntaxTheme::GrokNight);
    let full = highlight_code_spans("fn zed() {}\n", Some("rust"));
    assert_eq!(full.len(), 1);
    let part = highlight_code_spans("fn zed() {}", Some("rust"));
    assert_eq!(part.len(), 1);
    let unk = highlight_code_spans("plain body", Some("not-a-language"));
    assert_eq!(unk[0][0].1, "plain body");
}

#[test]
fn theme_tokyo_and_night_id() {
    let _theme = lock_theme();
    set_syntax_theme(SyntaxTheme::GrokNight);
    assert_eq!(syntax_theme(), SyntaxTheme::GrokNight);
    set_syntax_theme(SyntaxTheme::TokyoNight);
    assert_eq!(syntax_theme(), SyntaxTheme::TokyoNight);
    let _ = theme();
    set_syntax_theme(SyntaxTheme::TokyoNight);
    set_syntax_theme(SyntaxTheme::GrokNight);
}

#[test]
fn empty_language_and_empty_code() {
    let plain = highlight_code("fn x() {}", "   ");
    assert_eq!(plain, "fn x() {}");
    let empty = highlight_code_spans("", None);
    assert!(empty.is_empty() || empty.iter().all(|l| l.is_empty() || l[0].1.is_empty()));
}

#[test]
fn closed_cache_hit_and_reinsert() {
    let _theme = lock_theme();
    set_syntax_theme(SyntaxTheme::GrokNight);
    let code = "fn cache_hit() {}\n";
    let a = highlight_code_spans(code, Some("rust"));
    let b = highlight_code_spans(code, Some("rust"));
    assert_eq!(a, b);

    let mut cache = ClosedHighlightCache::default();
    let v = Arc::new(vec![vec![((1, 1, 1), String::from("x"))]]);
    cache.insert(7, Arc::clone(&v));
    cache.insert(7, Arc::clone(&v));
    assert_eq!(cache.len(), 1);
    let orphan = cache.evict_with_empty_order();
    let _ = cache.get(&orphan);
}

#[test]
fn styles_helpers_and_stream_replay() {
    assert_eq!(
        styles_to_spans(std::iter::empty::<(Style, &str)>()).len(),
        1
    );
    let plain = styles_or_plain(None, "line\n");
    assert_eq!(plain[0].1, "line");
    let styled = styles_or_plain(Some(Vec::new()), "x");
    assert_eq!(styled.len(), 1);

    let mut stream = OpenStreamHighlighter::new();
    let full = "let a = 1;\n";
    let _ = stream.highlight(full, Some("rust")).unwrap();
    let again = stream.highlight(full, Some("rust")).unwrap();
    assert_eq!(again.len(), 1);
    note_parse(true, &mut stream);
    note_parse(false, &mut stream);
    assert_eq!(spans_or_blank(Vec::new()).len(), 1);
    assert_eq!(spans_or_blank(vec![((1, 2, 3), "x".into())])[0].1, "x");

    let mut s2 = OpenStreamHighlighter::new();
    let _ = s2.highlight("partial", Some("rust")).unwrap();
}

#[test]
fn detect_language_covers_filenames_and_extensions() {
    let names = [
        ("Dockerfile", "dockerfile"),
        ("Makefile", "makefile"),
        ("GNUmakefile", "makefile"),
        ("CMakeLists.txt", "cmake"),
        ("Cargo.toml", "toml"),
        ("pyproject.toml", "toml"),
    ];
    for (path, lang) in names {
        assert_eq!(detect_language(path), Some(lang), "{path}");
    }

    let exts = [
        ("a.rs", "rust"),
        ("a.py", "python"),
        ("a.js", "javascript"),
        ("a.mjs", "javascript"),
        ("a.cjs", "javascript"),
        ("a.jsx", "jsx"),
        ("a.ts", "typescript"),
        ("a.mts", "typescript"),
        ("a.cts", "typescript"),
        ("a.tsx", "tsx"),
        ("a.html", "html"),
        ("a.htm", "html"),
        ("a.css", "css"),
        ("a.scss", "scss"),
        ("a.json", "json"),
        ("a.jsonc", "json"),
        ("a.toml", "toml"),
        ("a.yaml", "yaml"),
        ("a.yml", "yaml"),
        ("a.md", "markdown"),
        ("a.markdown", "markdown"),
        ("a.sh", "bash"),
        ("a.bash", "bash"),
        ("a.zsh", "bash"),
        ("a.sql", "sql"),
        ("a.c", "c"),
        ("a.h", "c"),
        ("a.cpp", "c++"),
        ("a.cc", "c++"),
        ("a.cxx", "c++"),
        ("a.hpp", "c++"),
        ("a.hh", "c++"),
        ("a.go", "go"),
        ("a.java", "java"),
        ("a.rb", "ruby"),
        ("a.swift", "swift"),
        ("a.kt", "kotlin"),
        ("a.kts", "kotlin"),
        ("a.scala", "scala"),
        ("a.r", "r"),
        ("a.lua", "lua"),
        ("a.php", "php"),
        ("a.xml", "xml"),
        ("a.svg", "xml"),
        ("a.vue", "vue"),
        ("a.svelte", "svelte"),
        ("a.dockerfile", "dockerfile"),
        ("a.makefile", "makefile"),
        ("a.mk", "makefile"),
        ("a.zig", "zig"),
        ("a.ex", "elixir"),
        ("a.exs", "elixir"),
        ("a.hs", "haskell"),
        ("a.nim", "nim"),
        ("a.dart", "dart"),
        ("a.proto", "protobuf"),
        ("a.graphql", "graphql"),
        ("a.gql", "graphql"),
        ("a.tf", "hcl"),
        ("a.hcl", "hcl"),
        ("a.nix", "nix"),
        ("a.vim", "vim"),
        ("a.diff", "diff"),
        ("a.patch", "diff"),
    ];
    for (path, lang) in exts {
        assert_eq!(detect_language(path), Some(lang), "{path}");
    }

    assert_eq!(detect_language(""), None);
    assert_eq!(detect_language("/"), None);
    assert_eq!(detect_language("noext"), None);
    assert_eq!(detect_language("dir/file.unknown"), None);
}

#[test]
fn find_syntax_aliases_and_first_line() {
    let ps = syntax_set();
    assert!(find_syntax(ps, None).is_none());
    assert!(find_syntax(ps, Some("")).is_none());
    assert!(find_syntax(ps, Some("   ")).is_none());
    assert!(find_syntax(ps, Some("not-a-language")).is_none());

    // Uppercase so token/extension lookup often misses and the alias match runs.
    // Default syntect pack does not include every alias target (e.g. typescript).
    for alias in [
        "RS",
        "PY",
        "JS",
        "MJS",
        "CJS",
        "TS",
        "MTS",
        "CTS",
        "TSX",
        "JSX",
        "YML",
        "SH",
        "ZSH",
        "SHELL",
        "DOCKERFILE",
        "KT",
        "KTS",
        "CS",
        "CPP",
        "CC",
        "CXX",
        "HPP",
        "rs",
        "mjs",
        "cjs",
        "shell",
        "yml",
        "kt",
    ] {
        let _ = find_syntax(ps, Some(alias));
    }

    let shebang = "#!/usr/bin/env python3\nprint(1)\n";
    assert!(find_syntax_for(ps, None, shebang).is_some());
    let skipped_blank = "\n\n#!/bin/bash\necho hi\n";
    assert!(find_syntax_for(ps, None, skipped_blank).is_some());
    assert!(find_syntax_for(ps, None, "\n\n").is_none());
    assert!(find_syntax_for(ps, None, "").is_none());

    let _ = highlight_code(shebang, "");
    let _ = highlight_code_spans(shebang, None);
    let _ = highlight_uncached("\n\n", None);
}
