//! Syntax highlighting via syntect (+ optional two-face).
//!
//! Stack mirrors Grok's markdown renderer:
//! - **syntect** — TextMate grammars and themes (defaults pack unless
//!   `extended-syntax` enables two-face / bat's ~250+ languages)
//! - **Grok Night / Grok Day / Tokyo Night** — same `.tmTheme` files Grok
//!   picks from the active TUI theme (`set_syntax_theme`)
//! - **Open-stream cache** — resumable `ParseState` / `HighlightState` so a
//!   growing fenced block is highlighted O(new lines) per frame, not O(N²)
//!
//! Grammar/theme data is loaded once (`OnceLock`). Stable (closed) blocks are
//! also memoised by content hash for exact re-renders.

use std::io::Cursor;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, LockResult, Mutex, MutexGuard, OnceLock, PoisonError};

use syntect::easy::HighlightLines;
use syntect::highlighting::{
    HighlightIterator, HighlightState, Highlighter, Style, Theme, ThemeSet,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};
use syntect::util::{LinesWithEndings, as_24_bit_terminal_escaped};

/// Fallback foreground when a language is unknown (light grey on dark UIs).
const PLAIN_FG: (u8, u8, u8) = (0xcc, 0xcc, 0xcc);

pub(crate) fn recover_lock<T>(r: LockResult<MutexGuard<'_, T>>) -> MutexGuard<'_, T> {
    r.unwrap_or_else(PoisonError::into_inner)
}

/// One highlighted run of code: 24-bit colour and the text it applies to.
pub type CodeSpan = ((u8, u8, u8), String);

/// Syntax definitions, loaded once.
///
/// Uses the newline-aware set so multi-line strings and comments stay correct
/// when highlighting with [`LinesWithEndings`].
///
/// - **default:** syntect's built-in pack (~360 KiB) — common languages only.
/// - **`extended-syntax`:** two-face / bat pack (~900 KiB, 250+ languages).
pub fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(|| {
        #[cfg(feature = "extended-syntax")]
        {
            two_face::syntax::extra_newlines()
        }
        #[cfg(not(feature = "extended-syntax"))]
        {
            SyntaxSet::load_defaults_newlines()
        }
    })
}

/// Which Grok syntax theme paints fenced code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SyntaxTheme {
    /// Default Grok Night (dark UIs).
    GrokNight = 0,
    /// Grok Day (light UIs).
    GrokDay = 1,
    /// Tokyo Night (only when the TUI theme is Tokyo Night).
    TokyoNight = 2,
}

impl SyntaxTheme {
    fn from_id(id: u8) -> Self {
        match id {
            1 => Self::GrokDay,
            2 => Self::TokyoNight,
            _ => Self::GrokNight,
        }
    }
}

static ACTIVE_SYNTAX_THEME: AtomicU8 = AtomicU8::new(SyntaxTheme::GrokNight as u8);

/// Active syntax theme (Grok Night unless the TUI selected another).
pub fn syntax_theme() -> SyntaxTheme {
    SyntaxTheme::from_id(ACTIVE_SYNTAX_THEME.load(Ordering::Relaxed))
}

/// Switch the syntax theme. Clears highlight caches so the next paint remaps.
pub fn set_syntax_theme(kind: SyntaxTheme) {
    let prev = ACTIVE_SYNTAX_THEME.swap(kind as u8, Ordering::Relaxed);
    if prev == kind as u8 {
        return;
    }
    recover_lock(closed_cache().lock()).clear();
    *recover_lock(open_stream().lock()) = OpenStreamHighlighter::new();
}

fn load_embedded_theme(bytes: &'static [u8]) -> Theme {
    let mut cursor = Cursor::new(bytes);
    ThemeSet::load_from_reader(&mut cursor).expect("embedded tmTheme must parse")
}

fn grok_night_theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| load_embedded_theme(include_bytes!("../assets/grok-night.tmTheme")))
}

fn grok_day_theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| load_embedded_theme(include_bytes!("../assets/grok-day.tmTheme")))
}

fn tokyo_night_theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| load_embedded_theme(include_bytes!("../assets/tokyo-night.tmTheme")))
}

/// Active TextMate theme (Grok Night by default — same as Grok Build).
pub fn theme() -> &'static Theme {
    match syntax_theme() {
        SyntaxTheme::GrokNight => grok_night_theme(),
        SyntaxTheme::GrokDay => grok_day_theme(),
        SyntaxTheme::TokyoNight => tokyo_night_theme(),
    }
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

/// Token lookup, then shebang / first-line guess for untagged fences.
fn find_syntax_for<'a>(
    ps: &'a SyntaxSet,
    language: Option<&str>,
    code: &str,
) -> Option<&'a syntect::parsing::SyntaxReference> {
    find_syntax(ps, language).or_else(|| {
        let first = code.lines().find(|l| !l.trim().is_empty())?;
        ps.find_syntax_by_first_line(first)
    })
}

/// Highlight source code with ANSI terminal escape sequences.
///
/// Unknown languages return the code unchanged (no fences added).
pub fn highlight_code(code: &str, language: &str) -> String {
    let ps = syntax_set();
    let Some(syntax) = find_syntax_for(ps, Some(language), code) else {
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
///
/// Returns [`Arc`] so a closed-cache hit is O(1) pointer clone rather than
/// deep-copying every span every frame (the TUI pays this per visible block).
pub fn highlight_code_spans(code: &str, language: Option<&str>) -> Arc<Vec<Vec<CodeSpan>>> {
    let key = cache_key(code, language);
    if let Some(hit) = recover_lock(closed_cache().lock()).get(&key) {
        return hit;
    }

    // Known languages go through the open-stream highlighter so a growing
    // fence pays O(new lines) instead of re-tokenising the whole body.
    if let Some((lines, committed)) = with_open_code_spans(code, language, |committed, partial| {
        let mut lines = Vec::with_capacity(committed.len() + usize::from(partial.is_some()));
        lines.extend(committed.iter().cloned());
        if let Some(p) = partial {
            lines.push(p.to_vec());
        }
        // Same as `OpenStreamHighlighter::is_fully_committed`: no partial
        // tail means every byte is a committed newline-terminated line.
        let fully = partial.is_none() && (code.is_empty() || code.ends_with('\n'));
        (lines, fully)
    }) {
        let arc = Arc::new(lines);
        if committed {
            insert_closed(key, Arc::clone(&arc));
        }
        return arc;
    }

    let computed = Arc::new(highlight_uncached(code, language));
    insert_closed(key, Arc::clone(&computed));
    computed
}

/// Most highlighted blocks held at once in the closed-content memo.
const CACHE_ENTRIES: usize = 64;

/// Bounded LRU of finished (fully committed) highlight results.
///
/// A full `clear()` at capacity used to evict *every* stable fence the
/// first time a session painted the 65th distinct block — the next scroll
/// frame then re-tokenised the visible ones. Evict only the oldest.
#[derive(Default)]
struct ClosedHighlightCache {
    map: rustc_hash::FxHashMap<u64, Arc<Vec<Vec<CodeSpan>>>>,
    /// Oldest at the front.
    order: std::collections::VecDeque<u64>,
}

impl ClosedHighlightCache {
    fn get(&mut self, key: &u64) -> Option<Arc<Vec<Vec<CodeSpan>>>> {
        let hit = self.map.get(key).cloned()?;
        if let Some(i) = self.order.iter().position(|k| k == key) {
            self.order.remove(i);
        }
        self.order.push_back(*key);
        Some(hit)
    }

    fn insert(&mut self, key: u64, value: Arc<Vec<Vec<CodeSpan>>>) {
        let existed = self.map.insert(key, value).is_some();
        if existed {
            if let Some(i) = self.order.iter().position(|k| *k == key) {
                self.order.remove(i);
            }
            self.order.push_back(key);
            return;
        }
        while self.map.len() > CACHE_ENTRIES {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            } else {
                break;
            }
        }
        self.order.push_back(key);
    }

    fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }

    #[cfg(test)]
    fn evict_with_empty_order(&mut self) -> u64 {
        self.order.clear();
        let orphan = 10_000u64;
        self.map.insert(orphan, Arc::new(vec![]));
        while self.map.len() <= CACHE_ENTRIES {
            let k = 10_000 + self.map.len() as u64;
            self.map.insert(k, Arc::new(vec![]));
        }
        self.insert(orphan, Arc::new(vec![]));
        self.insert(99_999, Arc::new(vec![]));
        orphan
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.map.len()
    }
}

type HighlightCache = Mutex<ClosedHighlightCache>;

fn closed_cache() -> &'static HighlightCache {
    static CACHE: OnceLock<HighlightCache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn insert_closed(key: u64, value: Arc<Vec<Vec<CodeSpan>>>) {
    recover_lock(closed_cache().lock()).insert(key, value);
}

fn cache_key(code: &str, language: Option<&str>) -> u64 {
    use std::hash::{Hash, Hasher};
    // FxHash: trusted local memo keys, hashed every frame on the TUI path.
    // SipHash (DefaultHasher) is for untrusted map keys — wrong tradeoff here.
    let mut hasher = rustc_hash::FxHasher::default();
    code.hash(&mut hasher);
    language.hash(&mut hasher);
    (syntax_theme() as u8).hash(&mut hasher);
    // Bump if grammar pack changes.
    #[cfg(feature = "extended-syntax")]
    "two-face-v1".hash(&mut hasher);
    #[cfg(not(feature = "extended-syntax"))]
    "syntect-defaults-v1".hash(&mut hasher);
    hasher.finish()
}

fn highlight_uncached(code: &str, language: Option<&str>) -> Vec<Vec<CodeSpan>> {
    let ps = syntax_set();

    let Some(syntax) = find_syntax_for(ps, language, code) else {
        return plain_lines(code);
    };

    let mut highlighter = HighlightLines::new(syntax, theme());
    LinesWithEndings::from(code)
        .map(|line| styles_or_plain(highlighter.highlight_line(line, ps).ok(), line))
        .collect()
}

pub(crate) fn styles_or_plain(ranges: Option<Vec<(Style, &str)>>, line: &str) -> Vec<CodeSpan> {
    match ranges {
        Some(ranges) => styles_to_spans(ranges),
        None => vec![(PLAIN_FG, line.trim_end_matches(['\r', '\n']).to_string())],
    }
}

pub(crate) fn styles_to_spans<'a, I>(ranges: I) -> Vec<CodeSpan>
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

pub(crate) fn spans_or_blank(spans: Vec<CodeSpan>) -> Vec<CodeSpan> {
    if spans.is_empty() {
        vec![(PLAIN_FG, String::new())]
    } else {
        spans
    }
}

fn note_parse(ok: bool, stream: &mut OpenStreamHighlighter) {
    if !ok {
        stream.invalidate_parse();
    }
}

fn open_stream() -> &'static Mutex<OpenStreamHighlighter> {
    static STREAM: OnceLock<Mutex<OpenStreamHighlighter>> = OnceLock::new();
    STREAM.get_or_init(|| Mutex::new(OpenStreamHighlighter::new()))
}

/// Visit highlighted lines of a growing fence without cloning the committed
/// prefix. The TUI open-fence painter uses this so a 500-line stream is
/// O(new lines) per frame rather than O(N) span clones.
///
/// `f` receives (committed newline-terminated lines, optional partial tail).
pub fn with_open_code_spans<R>(
    code: &str,
    language: Option<&str>,
    f: impl FnOnce(&[Vec<CodeSpan>], Option<&[CodeSpan]>) -> R,
) -> Option<R> {
    find_syntax_for(syntax_set(), language, code)?;
    let mut stream = recover_lock(open_stream().lock());
    stream.highlight_in_place(code, language)?;
    Some(f(&stream.committed_lines, stream.partial.as_deref()))
}

/// Resumable syntect state for the hot (usually trailing, still-open) fence.
struct OpenStreamHighlighter {
    /// Language token this state was built for (`None` = plain / unset).
    language: Option<String>,
    /// Source bytes committed so far (only complete, newline-terminated lines).
    committed_source: String,
    /// Highlighted lines corresponding to `committed_source`.
    committed_lines: Vec<Vec<CodeSpan>>,
    /// Trailing partial line (no newline yet). Not committed.
    partial: Option<Vec<CodeSpan>>,
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
            partial: None,
            parse_state: ParseState::new(ps.find_syntax_plain_text()),
            highlight_state: HighlightState::new(&highlighter, ScopeStack::new()),
        }
    }

    /// Whether every byte of `code` is in the committed prefix (no partial tail).
    #[cfg(test)]
    fn is_fully_committed(&self, code: &str) -> bool {
        self.committed_source == code && self.partial.is_none()
    }

    fn invalidate_parse(&mut self) {
        self.language = None;
        self.committed_source.clear();
        self.committed_lines.clear();
        self.partial = None;
    }

    /// Highlight `code` for `language`, reusing state when the body grew by
    /// append only. Returns `None` only if the language has no syntax (caller
    /// should fall back to plain text).
    #[cfg(test)]
    fn highlight(&mut self, code: &str, language: Option<&str>) -> Option<Vec<Vec<CodeSpan>>> {
        self.highlight_in_place(code, language)?;
        let mut out = self.committed_lines.clone();
        if let Some(last) = &self.partial {
            out.push(last.clone());
        }
        Some(out)
    }

    /// Advance parse state in place. Committed lines stay put so a TUI
    /// visitor can read them without an O(N) clone of every span.
    fn highlight_in_place(&mut self, code: &str, language: Option<&str>) -> Option<()> {
        let ps = syntax_set();
        let syntax = find_syntax_for(ps, language, code)?;

        let lang_key = language.map(|s| s.to_string());
        let needs_rebuild = self.language != lang_key || !code.starts_with(&self.committed_source);

        if needs_rebuild {
            let highlighter = Highlighter::new(theme());
            self.language = lang_key;
            self.committed_source.clear();
            self.committed_lines.clear();
            self.partial = None;
            self.parse_state = ParseState::new(syntax);
            self.highlight_state = HighlightState::new(&highlighter, ScopeStack::new());
        }

        self.partial = None;

        // Nothing new past the last committed newline.
        if self.committed_source.len() == code.len() {
            return Some(());
        }

        let highlighter = Highlighter::new(theme());

        for line in LinesWithEndings::from(&code[self.committed_source.len()..]) {
            if line.ends_with('\n') {
                let parsed = self.parse_state.parse_line(line, ps);
                note_parse(parsed.is_ok(), self);
                let ops = parsed.ok()?;
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
                self.committed_lines.push(spans_or_blank(spans));
                self.committed_source.push_str(line);
            } else {
                // Trailing partial line: clone state, do not commit.
                let mut parse_state = self.parse_state.clone();
                let mut highlight_state = self.highlight_state.clone();
                let parsed = parse_state.parse_line(line, ps);
                note_parse(parsed.is_ok(), self);
                let ops = parsed.ok()?;
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
                self.partial = Some(spans_or_blank(spans));
            }
        }

        Some(())
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
#[path = "highlight_tests.rs"]
mod tests;
