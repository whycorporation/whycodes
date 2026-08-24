//! Standalone prose keywords that inject a hidden per-turn instruction.
//!
//! Matching is conservative so identifiers, paths, and code samples do not
//! change agent behaviour. Notices are request-only (not stored in the
//! session transcript).

use whycode_config::MagicKeywordsConfig;

const ULTRATHINK: &str = "ultrathink";
const ORCHESTRATE: &str = "orchestrate";

pub const ULTRATHINK_NOTICE: &str = "\n\n<whycode_keyword name=\"ultrathink\">\n\
Think carefully before acting. Work through the problem in multiple steps: \
clarify the goal, consider failure modes and edge cases, then choose an \
approach. Prefer a correct, durable change over a fast one.\n\
</whycode_keyword>";

pub const ORCHESTRATE_NOTICE: &str = "\n\n<whycode_keyword name=\"orchestrate\">\n\
This is substantial independent work. Scope the full task first, then delegate \
parallel pieces with the `task` / `swarm` tools when they do not share files. \
Verify each phase (build, test, review) before continuing. Do not stop until \
the original request is complete.\n\
</whycode_keyword>";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MagicHit {
    pub ultrathink: bool,
    pub orchestrate: bool,
}

impl MagicHit {
    pub fn any(self) -> bool {
        self.ultrathink || self.orchestrate
    }

    pub fn notice(self) -> String {
        let mut out = String::new();
        if self.ultrathink {
            out.push_str(ULTRATHINK_NOTICE);
        }
        if self.orchestrate {
            out.push_str(ORCHESTRATE_NOTICE);
        }
        out
    }
}

pub fn scan(text: &str, cfg: &MagicKeywordsConfig) -> MagicHit {
    if !cfg.enabled {
        return MagicHit::default();
    }
    let masked = mask_non_prose(text);
    MagicHit {
        ultrathink: cfg.ultrathink && contains_keyword(&masked, ULTRATHINK),
        orchestrate: cfg.orchestrate && contains_keyword(&masked, ORCHESTRATE),
    }
}

fn contains_keyword(text: &str, keyword: &str) -> bool {
    let bytes = text.as_bytes();
    let needle = keyword.as_bytes();
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle && is_standalone(bytes, i, needle.len()) {
            return true;
        }
        i += 1;
    }
    false
}

fn is_standalone(bytes: &[u8], start: usize, len: usize) -> bool {
    let before_ok = start == 0 || is_boundary_before(bytes[start - 1] as char);
    let end = start + len;
    if !before_ok {
        return false;
    }
    if end == bytes.len() {
        return true;
    }
    let after = bytes[end] as char;
    if after == '.' {
        // Sentence period vs `keyword.ts` file extension.
        let next = bytes.get(end + 1).copied().map(|b| b as char);
        return !next.is_some_and(|c| c.is_ascii_alphanumeric());
    }
    is_boundary_after(after)
}

fn is_boundary_before(c: char) -> bool {
    !c.is_ascii_alphanumeric() && !matches!(c, '_' | '/' | '\\' | '-' | '.' | ':')
}

fn is_boundary_after(c: char) -> bool {
    !c.is_ascii_alphanumeric() && !matches!(c, '_' | '/' | '\\' | '-' | '.' | '(')
}

/// Replace fenced code, inline code, and HTML/XML markup with spaces so
/// keywords inside them cannot match while offsets stay aligned.
fn mask_non_prose(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        if fence_open(&chars, i) {
            let tick = chars[i];
            let mut j = i;
            while j < n && chars[j] == tick {
                j += 1;
            }
            let run = j - i;
            while i < j {
                out.push(' ');
                i += 1;
            }
            while i < n {
                if fence_close(&chars, i, tick, run) {
                    let end = i + run;
                    while i < end {
                        out.push(' ');
                        i += 1;
                    }
                    break;
                }
                out.push(if chars[i] == '\n' { '\n' } else { ' ' });
                i += 1;
            }
            continue;
        }
        if chars[i] == '`' {
            out.push(' ');
            i += 1;
            while i < n && chars[i] != '`' && chars[i] != '\n' {
                out.push(' ');
                i += 1;
            }
            if i < n && chars[i] == '`' {
                out.push(' ');
                i += 1;
            }
            continue;
        }
        if chars[i] == '<' {
            let is_close = i + 1 < n && chars[i + 1] == '/';
            while i < n {
                out.push(if chars[i] == '\n' { '\n' } else { ' ' });
                let done = chars[i] == '>';
                i += 1;
                if done {
                    break;
                }
            }
            if !is_close {
                while i < n {
                    if chars[i] == '<' && i + 1 < n && chars[i + 1] == '/' {
                        while i < n {
                            out.push(if chars[i] == '\n' { '\n' } else { ' ' });
                            let done = chars[i] == '>';
                            i += 1;
                            if done {
                                break;
                            }
                        }
                        break;
                    }
                    out.push(if chars[i] == '\n' { '\n' } else { ' ' });
                    i += 1;
                }
            }
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn fence_open(chars: &[char], i: usize) -> bool {
    let c = chars[i];
    (c == '`' || c == '~') && i + 2 < chars.len() && chars[i + 1] == c && chars[i + 2] == c
}

fn fence_close(chars: &[char], i: usize, tick: char, run: usize) -> bool {
    if i + run > chars.len() {
        return false;
    }
    chars[i..i + run].iter().all(|&c| c == tick)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> MagicKeywordsConfig {
        MagicKeywordsConfig::default()
    }

    #[test]
    fn hits_standalone_lowercase_words() {
        let hit = scan("please ultrathink about this", &on());
        assert!(hit.ultrathink);
        assert!(!hit.orchestrate);
        assert!(hit.any());

        let hit = scan("orchestrate the migration, then stop", &on());
        assert!(hit.orchestrate);
        assert!(!hit.ultrathink);
    }

    #[test]
    fn both_keywords_in_one_prompt() {
        let hit = scan("ultrathink then orchestrate the rollout", &on());
        assert!(hit.ultrathink && hit.orchestrate);
        let notice = hit.notice();
        assert!(notice.contains("ultrathink"));
        assert!(notice.contains("orchestrate"));
    }

    #[test]
    fn ignores_identifiers_paths_and_calls() {
        for sample in [
            "Ultrathink about this",
            "orchestrated the change",
            "see orchestrate.ts",
            "foo::orchestrate",
            "call orchestrate()",
            "path/ultrathink/file",
            "ultrathink-mode",
        ] {
            let hit = scan(sample, &on());
            assert!(!hit.any(), "should not match: {sample}");
        }
    }

    #[test]
    fn ignores_code_spans_and_fences() {
        assert!(!scan("use `ultrathink` here", &on()).any());
        assert!(!scan("```\nultrathink\n```\nok", &on()).ultrathink);
        assert!(!scan("~~~\norchestrate\n~~~\n", &on()).orchestrate);
        assert!(scan("```\ncode\n```\nultrathink", &on()).ultrathink);
        assert!(!scan("<note>ultrathink</note>", &on()).ultrathink);
    }

    #[test]
    fn punctuation_may_touch_the_word() {
        assert!(scan("ultrathink.", &on()).ultrathink);
        assert!(scan("\"orchestrate\"", &on()).orchestrate);
        assert!(scan("(ultrathink)", &on()).ultrathink);
    }

    #[test]
    fn config_switches_disable_notices() {
        let mut cfg = on();
        cfg.enabled = false;
        assert!(!scan("ultrathink please", &cfg).any());

        cfg = on();
        cfg.ultrathink = false;
        let hit = scan("ultrathink and orchestrate", &cfg);
        assert!(!hit.ultrathink);
        assert!(hit.orchestrate);
    }

    #[test]
    fn empty_notice_when_nothing_matched() {
        assert!(MagicHit::default().notice().is_empty());
        assert!(!MagicHit::default().any());
    }
}
