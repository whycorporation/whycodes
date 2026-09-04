//! Session display titles — defaults, heuristics, and sanitization.
//!
//! Industry pattern (Claude Code / OpenCode): cheap placeholder at create time,
//! then a short human-readable title once the conversation has content.
//! WhyCodes goes further with an instant offline heuristic before any LLM call.

use std::path::Path;

/// Where the current session title came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitleSource {
    /// `{project}-{ab}` placeholder assigned at creation.
    #[default]
    Default,
    /// Offline title derived from the first user message.
    Heuristic,
    /// Title produced by a small/fast model.
    Generated,
    /// User set the name (`/rename`, CLI rename, `--title`).
    Manual,
}

impl TitleSource {
    /// May replace with a first-message heuristic.
    pub fn allows_heuristic(self) -> bool {
        matches!(self, Self::Default)
    }

    /// May upgrade via a small-model title call.
    pub fn allows_llm(self) -> bool {
        matches!(self, Self::Default | Self::Heuristic)
    }
}

/// Default display name: project basename + two hex chars from the session id.
///
/// Example: `whycodes-a3`. Matches Claude Code's `my-app-3f` style — scannable
/// in a list of live sessions without waiting for an LLM.
pub fn default_title(project_path: &Path, session_id: &str) -> String {
    let base = project_basename(project_path);
    let suffix = short_id_suffix(session_id);
    format!("{base}-{suffix}")
}

fn project_basename(project_path: &Path) -> String {
    project_path
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && *s != "/" && *s != ".")
        .unwrap_or("session")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .chars()
        .take(32)
        .collect::<String>()
        .pipe(|s| if s.is_empty() { "session".into() } else { s })
}

/// Tiny helper so we can keep the basename pipeline readable.
trait Pipe: Sized {
    fn pipe<F, R>(self, f: F) -> R
    where
        F: FnOnce(Self) -> R;
}

impl<T> Pipe for T {
    fn pipe<F, R>(self, f: F) -> R
    where
        F: FnOnce(Self) -> R,
    {
        f(self)
    }
}

fn short_id_suffix(session_id: &str) -> String {
    let hex: String = session_id
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(2)
        .collect::<String>()
        .to_ascii_lowercase();
    if hex.len() == 2 { hex } else { "00".into() }
}

/// True when `title` still looks like an auto placeholder for this project.
pub fn looks_like_default_title(title: &str, project_path: &Path) -> bool {
    let t = title.trim();
    if t.starts_with("New session") {
        return true;
    }
    let base = project_basename(project_path);
    // `{base}-xx` where xx is two hex digits
    if let Some(rest) = t.strip_prefix(&format!("{base}-"))
        && rest.len() == 2
        && rest.chars().all(|c| c.is_ascii_hexdigit())
    {
        return true;
    }
    false
}

/// Instant offline title from the first user message.
///
/// Strips common filler, keeps 3–8 content words, caps length. Good enough for
/// the session picker before (or without) an LLM refine pass.
pub fn heuristic_title(text: &str) -> String {
    let raw = first_meaningful_line(text);
    let stripped = strip_leading_filler(raw);
    let words: Vec<&str> = stripped
        .split_whitespace()
        .filter(|w| !w.is_empty())
        .take(8)
        .collect();
    if words.is_empty() {
        return String::new();
    }
    // Prefer at least 3 words when available; allow fewer for short prompts.
    let take = words.len().clamp(1, 8);
    let joined = words[..take].join(" ");
    sanitize_title(&joined)
}

fn first_meaningful_line(text: &str) -> &str {
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        // Skip pure @path lines from expansions when a real prompt follows.
        if t.starts_with('@') && !t.contains(' ') && text.lines().count() > 1 {
            continue;
        }
        return t;
    }
    text.trim()
}

fn strip_leading_filler(s: &str) -> &str {
    const FILLERS: &[&str] = &[
        "please ",
        "pls ",
        "can you ",
        "could you ",
        "would you ",
        "help me ",
        "i need you to ",
        "i want you to ",
        "hey ",
        "hi ",
        "hello ",
    ];
    let mut lower = s.to_ascii_lowercase();
    let mut offset = 0usize;
    let bytes = s.as_bytes();
    loop {
        let mut hit = false;
        for f in FILLERS {
            if lower.starts_with(f) {
                offset += f.len();
                lower = lower[f.len()..].to_string();
                hit = true;
                break;
            }
        }
        if !hit {
            break;
        }
    }
    // Map byte offset back safely (fillers are ASCII).
    let start = offset.min(bytes.len());
    s.get(start..).unwrap_or(s).trim()
}

/// Normalize a model- or heuristic-produced title for storage/display.
pub fn sanitize_title(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    // Strip wrapping quotes the model often adds.
    if (s.starts_with('"') && s.ends_with('"'))
        || (s.starts_with('\'') && s.ends_with('\''))
        || (s.starts_with('“') && s.ends_with('”'))
    {
        s = s[1..s.len() - 1].trim().to_string();
    }
    // Drop a trailing period/exclamation (keep `?` rare titles intact? drop all).
    while s.ends_with(['.', '!', ':', ';', ',']) {
        s.pop();
        s = s.trim_end().to_string();
    }
    // Collapse whitespace / strip control chars.
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    // Hard cap — session pickers are narrow.
    const MAX_CHARS: usize = 64;
    let n = cleaned.chars().count();
    if n <= MAX_CHARS {
        cleaned
    } else {
        let mut out: String = cleaned.chars().take(MAX_CHARS.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Infer source when loading a row that predates `title_source` persistence.
pub fn infer_source_from_title(title: &str, project_path: &Path) -> TitleSource {
    if looks_like_default_title(title, project_path) {
        TitleSource::Default
    } else {
        // Already has a real name — do not thrash it on resume.
        TitleSource::Generated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn default_title_uses_basename_and_id() {
        let t = default_title(&PathBuf::from("/home/u/whycodes"), "a3f1c2d4-…");
        assert_eq!(t, "whycodes-a3");
    }

    #[test]
    fn heuristic_strips_filler_and_limits_words() {
        let t = heuristic_title("Please can you fix the stripe webhook timeout in payments.rs");
        assert!(t.to_ascii_lowercase().contains("stripe"));
        assert!(!t.to_ascii_lowercase().starts_with("please"));
        assert!(t.split_whitespace().count() <= 8);
    }

    #[test]
    fn sanitize_strips_quotes() {
        assert_eq!(sanitize_title("  \"Auth refactor\"  "), "Auth refactor");
    }

    #[test]
    fn looks_like_default() {
        let p = PathBuf::from("/tmp/my-app");
        assert!(looks_like_default_title("my-app-3f", &p));
        assert!(looks_like_default_title("New session - 2026-01-01", &p));
        assert!(!looks_like_default_title("Fix webhook timeout", &p));
    }

    #[test]
    fn source_gates() {
        assert!(TitleSource::Default.allows_heuristic());
        assert!(TitleSource::Default.allows_llm());
        assert!(!TitleSource::Heuristic.allows_heuristic());
        assert!(TitleSource::Heuristic.allows_llm());
        assert!(!TitleSource::Manual.allows_llm());
        assert!(!TitleSource::Generated.allows_llm());
    }

    #[test]
    fn defaults_handle_unusual_paths_and_ids() {
        assert_eq!(default_title(Path::new("/"), "x"), "session-00");
        assert_eq!(default_title(Path::new("/tmp/!!!"), "AB"), "session-ab");
        assert_eq!(
            default_title(Path::new("/tmp/a name"), "--fZ3"),
            "a-name-f3"
        );
        let long = "a".repeat(40);
        assert_eq!(default_title(Path::new(&long), "12").len(), 35);
        assert_eq!(default_title(Path::new("."), "zz"), "session-00");
        assert_eq!(project_basename(Path::new("")), "session");
        assert_eq!(short_id_suffix("zz"), "00");
    }

    #[test]
    fn heuristic_handles_empty_paths_and_multiline_prompts() {
        assert_eq!(heuristic_title(" \n\t"), "");
        assert_eq!(
            heuristic_title("@src/lib.rs\nPlease fix login now"),
            "fix login now"
        );
        assert_eq!(heuristic_title("@only.rs"), "@only.rs");
        assert_eq!(heuristic_title("Hello help me repair it"), "repair it");
    }

    #[test]
    fn sanitize_handles_quote_styles_punctuation_controls_and_caps() {
        assert_eq!(sanitize_title("'Single title'"), "Single title");
        assert_eq!(sanitize_title("Title!!!  "), "Title");
        assert_eq!(sanitize_title("\"Double title\""), "Double title");
        assert_eq!(sanitize_title("a\n\tb"), "a b");
        let long = "é".repeat(70);
        let cleaned = sanitize_title(&long);
        assert_eq!(cleaned.chars().count(), 64);
        assert!(cleaned.ends_with('…'));
    }

    #[test]
    fn default_detection_and_source_inference_cover_false_shapes() {
        let p = PathBuf::from("/tmp/my-app");
        assert!(!looks_like_default_title("other-3f", &p));
        assert!(!looks_like_default_title("my-app-z9", &p));
        assert!(!looks_like_default_title("my-app-123", &p));
        assert_eq!(
            infer_source_from_title("my-app-af", &p),
            TitleSource::Default
        );
        assert_eq!(
            infer_source_from_title("Useful title", &p),
            TitleSource::Generated
        );
    }
}
