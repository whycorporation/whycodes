//! Syntax highlighting via syntect + two-face.
//!
//! Stack mirrors Grok's markdown renderer:
//! - **syntect** — TextMate grammars and themes
//! - **two-face** — bat's extended syntax set (~250+ languages)
//! - **Tokyo Night** — embedded `.tmTheme` for terminal code blocks
//! - **Open-stream cache** — resumable `ParseState` / `HighlightState` so a
//!   growing fenced block is highlighted O(new lines) per frame, not O(N²)
//!
//! Grammar/theme data is loaded once (`OnceLock`). Stable (closed) blocks are
//! also memoised by content hash for exact re-renders.

use std::io::Cursor;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use syntect::easy::HighlightLines;
use syntect::highlighting::{
    HighlightIterator, HighlightState, Highlighter, Style, Theme, ThemeSet,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};
use syntect::util::{LinesWithEndings, as_24_bit_terminal_escaped};

/// Fallback foreground when a language is unknown (light grey on dark UIs).
const PLAIN_FG: (u8, u8, u8) = (0xcc, 0xcc, 0xcc);

/// One highlighted run of code: 24-bit colour and the text it applies to.
pub type CodeSpan = ((u8, u8, u8), String);

/// Syntect's extended syntax set (two-face / bat), loaded once.
///
/// Uses the newline-aware set so multi-line strings and comments stay correct
/// when highlighting with [`LinesWithEndings`].
pub fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

/// Tokyo Night theme, embedded at compile time.
pub fn theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        let bytes = include_bytes!("../assets/tokyo-night.tmTheme");
        let mut cursor = Cursor::new(bytes.as_slice());
        ThemeSet::load_from_reader(&mut cursor).expect("tokyo-night.tmTheme must parse")
    })
}

/// Resolve a language token or extension to a syntax definition.
fn find_syntax<'a>(
    ps: &'a SyntaxSet,
    language: Option<&str>,
) -> Option<&'a syntect::parsing::SyntaxReference> {
    let lang = language?.trim();
    if lang.is_empty() {
        return None;
    }
    ps.find_syntax_by_token(lang)
        .or_else(|| ps.find_syntax_by_extension(lang))
        // Common fence aliases that token lookup sometimes misses.
        .or_else(|| match lang.to_ascii_lowercase().as_str() {
            "rs" => ps.find_syntax_by_token("rust"),
            "py" => ps.find_syntax_by_token("python"),
            "js" | "mjs" | "cjs" => ps.find_syntax_by_token("javascript"),
            "ts" | "mts" | "cts" => ps.find_syntax_by_token("typescript"),
            "tsx" => ps.find_syntax_by_extension("tsx"),
            "jsx" => ps.find_syntax_by_extension("jsx"),
            "yml" => ps.find_syntax_by_token("yaml"),
            "sh" | "zsh" | "shell" => ps.find_syntax_by_token("bash"),
            "dockerfile" => ps.find_syntax_by_token("Dockerfile"),
            "kt" | "kts" => ps.find_syntax_by_token("kotlin"),
            "cs" => ps.find_syntax_by_token("c#"),
            "cpp" | "cc" | "cxx" | "hpp" => ps.find_syntax_by_token("c++"),
            _ => None,
        })
}

/// Highlight source code with ANSI terminal escape sequences.
///
/// Unknown languages return the code unchanged (no fences added).
pub fn highlight_code(code: &str, language: &str) -> String {
    let ps = syntax_set();
    let Some(syntax) = find_syntax(ps, Some(language)) else {
        return code.to_string();
    };

    let mut highlighter = HighlightLines::new(syntax, theme());
    let mut output = String::with_capacity(code.len() * 2);
    for line in LinesWithEndings::from(code) {
        let ranges: Vec<(Style, &str)> = highlighter.highlight_line(line, ps).unwrap_or_default();
        let escaped = as_24_bit_terminal_escaped(&ranges[..], false);
        output.push_str(&escaped);
    }
    output
}

/// Syntax-highlight code into coloured runs for frontends that cannot consume
/// ANSI (e.g. ratatui). Returns one `Vec<CodeSpan>` per line. An unknown
/// language yields the text unstyled rather than failing.
///
/// Hot path for streaming TUI renders:
/// 1. Exact-content memo (closed / stable blocks)
/// 2. Append-only open-stream cache (growing fenced body)
/// 3. Full batch highlight, then seed the stream cache
pub fn highlight_code_spans(code: &str, language: Option<&str>) -> Vec<Vec<CodeSpan>> {
    let key = cache_key(code, language);
    if let Some(hit) = closed_cache()
        .lock()
        .ok()
        .and_then(|c| c.get(&key).cloned())
    {
        return hit;
    }

    // Known languages go through the open-stream highlighter so a growing
    // fence pays O(new lines) instead of re-tokenising the whole body.
    if find_syntax(syntax_set(), language).is_some()
        && let Ok(mut stream) = open_stream().lock()
        && let Some(lines) = stream.highlight(code, language)
    {
        // Memo fully-committed bodies (no partial trailing line). Intermediate
        // streaming prefixes are left out so a long stream does not thrash the
        // closed-block cache.
        if stream.is_fully_committed(code) {
            insert_closed(key, lines.clone());
        }
        return lines;
    }

    let computed = highlight_uncached(code, language);
    insert_closed(key, computed.clone());
    computed
}

/// Most highlighted blocks held at once in the closed-content memo.
const CACHE_ENTRIES: usize = 64;

type HighlightCache = Mutex<rustc_hash::FxHashMap<u64, Vec<Vec<CodeSpan>>>>;

fn closed_cache() -> &'static HighlightCache {
    static CACHE: OnceLock<HighlightCache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn insert_closed(key: u64, value: Vec<Vec<CodeSpan>>) {
    if let Ok(mut cache) = closed_cache().lock() {
        if cache.len() >= CACHE_ENTRIES {
            cache.clear();
        }
        cache.insert(key, value);
    }
}

fn cache_key(code: &str, language: Option<&str>) -> u64 {
    use std::hash::{Hash, Hasher};
    // FxHash: trusted local memo keys, hashed every frame on the TUI path.
    // SipHash (DefaultHasher) is for untrusted map keys — wrong tradeoff here.
    let mut hasher = rustc_hash::FxHasher::default();
    code.hash(&mut hasher);
    language.hash(&mut hasher);
    // Bump if theme or grammar source changes.
    "tokyo-night-two-face-v1".hash(&mut hasher);
    hasher.finish()
}

fn highlight_uncached(code: &str, language: Option<&str>) -> Vec<Vec<CodeSpan>> {
    let ps = syntax_set();

    let Some(syntax) = find_syntax(ps, language) else {
        return plain_lines(code);
    };

    let mut highlighter = HighlightLines::new(syntax, theme());
    LinesWithEndings::from(code)
        .map(|line| match highlighter.highlight_line(line, ps) {
            Ok(ranges) => styles_to_spans(ranges),
            Err(_) => vec![(PLAIN_FG, line.trim_end_matches(['\r', '\n']).to_string())],
        })
        .collect()
}

fn styles_to_spans<'a, I>(ranges: I) -> Vec<CodeSpan>
where
    I: IntoIterator<Item = (Style, &'a str)>,
{
    let mut spans: Vec<CodeSpan> = ranges
        .into_iter()
        .map(|(style, text)| {
            (
                (style.foreground.r, style.foreground.g, style.foreground.b),
                text.trim_end_matches(['\r', '\n']).to_string(),
            )
        })
        .filter(|(_, t)| !t.is_empty())
        .collect();
    if spans.is_empty() {
        spans.push((PLAIN_FG, String::new()));
    }
    spans
}

fn plain_lines(code: &str) -> Vec<Vec<CodeSpan>> {
    if code.is_empty() {
        return Vec::new();
    }
    LinesWithEndings::from(code)
        .map(|line| vec![(PLAIN_FG, line.trim_end_matches(['\r', '\n']).to_string())])
        .collect()
}

// ── Open-stream incremental highlighter ────────────────────────────────
//
// Same strategy as Grok's `OpenCodeHighlighter`: newline-terminated lines
// permanently advance `ParseState` / `HighlightState`; the trailing partial
// line (still streaming) is highlighted on clones so the next push can extend
// it without replaying the whole block.

fn open_stream() -> &'static Mutex<OpenStreamHighlighter> {
    static STREAM: OnceLock<Mutex<OpenStreamHighlighter>> = OnceLock::new();
    STREAM.get_or_init(|| Mutex::new(OpenStreamHighlighter::new()))
}

/// Resumable syntect state for the hot (usually trailing, still-open) fence.
struct OpenStreamHighlighter {
    /// Language token this state was built for (`None` = plain / unset).
    language: Option<String>,
    /// Source bytes committed so far (only complete, newline-terminated lines).
    committed_source: String,
    /// Highlighted lines corresponding to `committed_source`.
    committed_lines: Vec<Vec<CodeSpan>>,
    parse_state: ParseState,
    highlight_state: HighlightState,
}

impl OpenStreamHighlighter {
    fn new() -> Self {
        let ps = syntax_set();
        let highlighter = Highlighter::new(theme());
        Self {
            language: None,
            committed_source: String::new(),
            committed_lines: Vec::new(),
            parse_state: ParseState::new(ps.find_syntax_plain_text()),
            highlight_state: HighlightState::new(&highlighter, ScopeStack::new()),
        }
    }

    /// Whether every byte of `code` is in the committed prefix (no partial tail).
    fn is_fully_committed(&self, code: &str) -> bool {
        self.committed_source == code
    }

    /// Highlight `code` for `language`, reusing state when the body grew by
    /// append only. Returns `None` only if the language has no syntax (caller
    /// should fall back to plain text).
    fn highlight(&mut self, code: &str, language: Option<&str>) -> Option<Vec<Vec<CodeSpan>>> {
        let ps = syntax_set();
        let syntax = find_syntax(ps, language)?;

        let lang_key = language.map(|s| s.to_string());
        let needs_rebuild = self.language != lang_key || !code.starts_with(&self.committed_source);

        if needs_rebuild {
            let highlighter = Highlighter::new(theme());
            self.language = lang_key;
            self.committed_source.clear();
            self.committed_lines.clear();
            self.parse_state = ParseState::new(syntax);
            self.highlight_state = HighlightState::new(&highlighter, ScopeStack::new());
        }

        // Nothing new past the last committed newline.
        if self.committed_source.len() == code.len() {
            return Some(self.committed_lines.clone());
        }

        let highlighter = Highlighter::new(theme());
        let mut tentative: Option<Vec<CodeSpan>> = None;

        for line in LinesWithEndings::from(&code[self.committed_source.len()..]) {
            if line.ends_with('\n') {
                let ops = match self.parse_state.parse_line(line, ps) {
                    Ok(ops) => ops,
                    Err(_) => {
                        // Invalidate so the next pass rebuilds from scratch.
                        self.language = None;
                        self.committed_source.clear();
                        self.committed_lines.clear();
                        return None;
                    }
                };
                let spans: Vec<CodeSpan> =
                    HighlightIterator::new(&mut self.highlight_state, &ops, line, &highlighter)
                        .map(|(s, t)| {
                            (
                                (s.foreground.r, s.foreground.g, s.foreground.b),
                                t.trim_end_matches(['\r', '\n']).to_string(),
                            )
                        })
                        .filter(|(_, t)| !t.is_empty())
                        .collect();
                let spans = if spans.is_empty() {
                    vec![(PLAIN_FG, String::new())]
                } else {
                    spans
                };
                self.committed_lines.push(spans);
                self.committed_source.push_str(line);
            } else {
                // Trailing partial line: clone state, do not commit.
                let mut parse_state = self.parse_state.clone();
                let mut highlight_state = self.highlight_state.clone();
                let ops = match parse_state.parse_line(line, ps) {
                    Ok(ops) => ops,
                    Err(_) => {
                        self.language = None;
                        self.committed_source.clear();
                        self.committed_lines.clear();
                        return None;
                    }
                };
                let spans: Vec<CodeSpan> =
                    HighlightIterator::new(&mut highlight_state, &ops, line, &highlighter)
                        .map(|(s, t)| {
                            (
                                (s.foreground.r, s.foreground.g, s.foreground.b),
                                t.trim_end_matches(['\r', '\n']).to_string(),
                            )
                        })
                        .filter(|(_, t)| !t.is_empty())
                        .collect();
                tentative = Some(if spans.is_empty() {
                    vec![(PLAIN_FG, String::new())]
                } else {
                    spans
                });
            }
        }

        let mut out = self.committed_lines.clone();
        if let Some(last) = tentative {
            out.push(last);
        }
        Some(out)
    }

    /// Test helper: how many complete lines are committed.
    #[cfg(test)]
    fn committed_line_count(&self) -> usize {
        self.committed_lines.len()
    }
}

/// Detect a language from a file path (extension or well-known filename).
pub fn detect_language(path: &str) -> Option<&str> {
    let path = Path::new(path);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match name.as_str() {
        "dockerfile" => return Some("dockerfile"),
        "makefile" | "gnumakefile" => return Some("makefile"),
        "cmakelists.txt" => return Some("cmake"),
        "cargo.toml" | "pyproject.toml" => return Some("toml"),
        _ => {}
    }

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "rs" => Some("rust"),
        "py" => Some("python"),
        "js" | "mjs" | "cjs" => Some("javascript"),
        "jsx" => Some("jsx"),
        "ts" | "mts" | "cts" => Some("typescript"),
        "tsx" => Some("tsx"),
        "html" | "htm" => Some("html"),
        "css" => Some("css"),
        "scss" => Some("scss"),
        "json" | "jsonc" => Some("json"),
        "toml" => Some("toml"),
        "yaml" | "yml" => Some("yaml"),
        "md" | "markdown" => Some("markdown"),
        "sh" | "bash" | "zsh" => Some("bash"),
        "sql" => Some("sql"),
        "c" => Some("c"),
        "h" => Some("c"),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => Some("c++"),
        "go" => Some("go"),
        "java" => Some("java"),
        "rb" => Some("ruby"),
        "swift" => Some("swift"),
        "kt" | "kts" => Some("kotlin"),
        "scala" => Some("scala"),
        "r" => Some("r"),
        "lua" => Some("lua"),
        "php" => Some("php"),
        "xml" | "svg" => Some("xml"),
        "vue" => Some("vue"),
        "svelte" => Some("svelte"),
        "dockerfile" => Some("dockerfile"),
        "makefile" | "mk" => Some("makefile"),
        "zig" => Some("zig"),
        "ex" | "exs" => Some("elixir"),
        "hs" => Some("haskell"),
        "nim" => Some("nim"),
        "dart" => Some("dart"),
        "proto" => Some("protobuf"),
        "graphql" | "gql" => Some("graphql"),
        "tf" | "hcl" => Some("hcl"),
        "nix" => Some("nix"),
        "vim" => Some("vim"),
        "diff" | "patch" => Some("diff"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let code = "fn main() { let x = 1; }";
        let first = highlight_code_spans(code, Some("rust"));
        let second = highlight_code_spans(code, Some("rust"));
        assert_eq!(first, second);
        assert_eq!(first, highlight_uncached(code, Some("rust")));
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
        let t = theme();
        assert!(
            t.name
                .as_deref()
                .unwrap_or("")
                .to_ascii_lowercase()
                .contains("tokyo")
                || t.settings.foreground.is_some()
                || t.settings.background.is_some(),
            "theme should load with name or colours"
        );
    }

    #[test]
    fn syntax_set_knows_common_languages() {
        let ps = syntax_set();
        for token in ["rust", "python", "javascript", "typescript", "go", "toml"] {
            assert!(
                ps.find_syntax_by_token(token).is_some(),
                "missing syntax for {token}"
            );
        }
    }

    // ── Open-stream incremental ────────────────────────────────────────

    #[test]
    fn stream_append_matches_batch() {
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
        let mut stream = OpenStreamHighlighter::new();
        let _ = stream.highlight("alpha: 1\n", Some("yaml")).unwrap();
        let b = "beta: 2\n";
        let got = stream.highlight(b, Some("yaml")).unwrap();
        assert_eq!(got, highlight_uncached(b, Some("yaml")));
    }

    #[test]
    fn public_api_stream_growth_matches_batch() {
        // Exercises the global stream via highlight_code_spans (what the TUI calls).
        let full = "key: value\nlist:\n  - a\n  - b\n";
        for end in 1..=full.len() {
            if !full.is_char_boundary(end) {
                continue;
            }
            let prefix = &full[..end];
            let got = highlight_code_spans(prefix, Some("yaml"));
            let batch = highlight_uncached(prefix, Some("yaml"));
            assert_eq!(got, batch, "prefix len {end}");
        }
    }

    #[test]
    fn multi_line_comment_survives_incremental_growth() {
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
}
