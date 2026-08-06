//! Shell command splitting.
//!
//! This is not a shell parser. It splits a command line into segments and
//! words well enough to ask "what does this touch", and marks the places where
//! it cannot know — command substitution, unbalanced quotes — so the caller can
//! escalate instead of guessing.

/// A single word of a command, after quote removal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    /// The word with quotes stripped. Variables are left intact: `$HOME` stays
    /// `$HOME`, because resolving it is the caller's decision.
    pub text: String,
    /// The word contained `$(…)` or a backtick substitution, so its real value
    /// is not knowable without running it.
    pub dynamic: bool,
}

impl Word {
    fn new(text: String, dynamic: bool) -> Self {
        Self { text, dynamic }
    }
}

/// One simple command, i.e. the part between `;`, `&&`, `||` or `|`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub words: Vec<Word>,
}

impl Segment {
    /// The command name, ignoring leading `VAR=value` assignments, `sudo` and
    /// shell control-flow keywords.
    pub fn command(&self) -> Option<&str> {
        self.words.iter().map(|w| w.text.as_str()).find(|w| {
            !is_assignment(w)
                && *w != "sudo"
                && *w != "command"
                && *w != "env"
                && !is_shell_keyword(w)
        })
    }

    /// Arguments after the command name.
    pub fn args(&self) -> impl Iterator<Item = &Word> {
        let cmd = self.command().map(str::to_string);
        let mut seen = false;
        self.words.iter().filter(move |w| {
            if seen {
                return true;
            }
            if Some(&w.text) == cmd.as_ref() {
                seen = true;
            }
            false
        })
    }

    /// True when any word could not be resolved statically.
    pub fn has_dynamic(&self) -> bool {
        self.words.iter().any(|w| w.dynamic)
    }
}

/// Shell control-flow keywords are transparent when looking for the command a
/// segment runs: in `then rm -rf ~`, the command is `rm`, not `then`.
///
/// Regression: jcode#725 — `if`/`while`/`case` bodies bypassed the guardrail
/// because the keyword sat in the command slot.
const SHELL_KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "do", "done", "while", "until", "for", "case", "esac",
    "in", "select", "time", "coproc", "!", "{", "}",
];

fn is_shell_keyword(word: &str) -> bool {
    SHELL_KEYWORDS.contains(&word)
}

fn is_assignment(word: &str) -> bool {
    match word.find('=') {
        Some(0) | None => false,
        Some(i) => word[..i]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_'),
    }
}

/// Result of splitting a command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tokenized {
    pub segments: Vec<Segment>,
    /// Quoting did not balance, so the split above is unreliable.
    pub malformed: bool,
}

/// Words that separate one simple command from the next.
/// Bare parentheses act as separators too: `( rm -rf ~ )` runs `rm` in a
/// subshell, and a `case` arm's `)` closes the pattern. Quoted or escaped
/// parens never reach this table, and `$(…)` is consumed earlier.
/// Regression: jcode#725 — subshell bodies bypassed the guardrail.
const SEPARATORS: &[&str] = &[";", "&&", "||", "|", "&", "\n", "(", ")"];

/// Redirection operators, kept as their own words so a target can follow.
const REDIRECTS: &[&str] = &[">", ">>", ">|", "&>", "&>>", "2>", "2>>", "1>", "1>>"];

/// True for a word that is a redirection operator.
pub fn is_redirect(word: &str) -> bool {
    REDIRECTS.contains(&word)
}

/// True for a redirection that truncates rather than appends.
pub fn is_truncating_redirect(word: &str) -> bool {
    matches!(word, ">" | ">|" | "&>" | "2>" | "1>")
}

/// Split a command line into segments of words.
pub fn tokenize(input: &str) -> Tokenized {
    let mut segments = Vec::new();
    let mut words: Vec<Word> = Vec::new();
    let mut current = String::new();
    let mut has_current = false;
    let mut dynamic = false;
    let mut malformed = false;

    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    macro_rules! flush_word {
        () => {
            if has_current {
                words.push(Word::new(std::mem::take(&mut current), dynamic));
                has_current = false;
                dynamic = false;
            }
        };
    }
    macro_rules! flush_segment {
        () => {
            flush_word!();
            if !words.is_empty() {
                segments.push(Segment {
                    words: std::mem::take(&mut words),
                });
            }
        };
    }

    while i < chars.len() {
        let c = chars[i];

        match c {
            '\\' if i + 1 < chars.len() => {
                current.push(chars[i + 1]);
                has_current = true;
                i += 2;
            }
            '\'' => {
                // Single quotes are literal all the way to the next quote.
                let Some(end) = find_char(&chars, i + 1, '\'') else {
                    malformed = true;
                    break;
                };
                current.extend(&chars[i + 1..end]);
                has_current = true;
                i = end + 1;
            }
            '"' => {
                // Double quotes allow substitution, so scan for it.
                let Some(end) = find_double_quote_end(&chars, i + 1) else {
                    malformed = true;
                    break;
                };
                let inner: String = chars[i + 1..end].iter().collect();
                if contains_substitution(&inner) {
                    dynamic = true;
                }
                current.push_str(&inner);
                has_current = true;
                i = end + 1;
            }
            '`' => {
                let Some(end) = find_char(&chars, i + 1, '`') else {
                    malformed = true;
                    break;
                };
                dynamic = true;
                has_current = true;
                i = end + 1;
            }
            '$' if i + 1 < chars.len() && chars[i + 1] == '(' => {
                let Some(end) = find_closing_paren(&chars, i + 2) else {
                    malformed = true;
                    break;
                };
                dynamic = true;
                has_current = true;
                i = end + 1;
            }
            // Process substitution: <(cmd) >(cmd)  — runs `cmd` and feeds a
            // pipe path. Mark dynamic so risk cannot pretend targets are static.
            // Also Zsh =(cmd) equals-form process substitution.
            '<' | '>' | '='
                if i + 1 < chars.len()
                    && chars[i + 1] == '('
                    && (c != '=' || is_process_sub_equals_context(&chars, i)) =>
            {
                let Some(end) = find_closing_paren(&chars, i + 2) else {
                    malformed = true;
                    break;
                };
                dynamic = true;
                has_current = true;
                // Keep a placeholder so the segment still has a word.
                current.push(c);
                current.push_str("()");
                i = end + 1;
            }
            c if c.is_whitespace() && c != '\n' => {
                flush_word!();
                i += 1;
            }
            _ => {
                // Separator or redirection operator?
                if let Some(op) = match_operator(&chars, i, SEPARATORS) {
                    flush_segment!();
                    i += op.chars().count();
                } else if let Some(op) = match_operator(&chars, i, REDIRECTS) {
                    flush_word!();
                    words.push(Word::new(op.to_string(), false));
                    i += op.chars().count();
                } else {
                    current.push(c);
                    has_current = true;
                    i += 1;
                }
            }
        }
    }

    // Final flush, written out rather than using the macros so the last
    // assignment to the state flags is not dead.
    if has_current {
        words.push(Word::new(current, dynamic));
    }
    if !words.is_empty() {
        segments.push(Segment { words });
    }

    Tokenized {
        segments,
        malformed,
    }
}

/// Match the longest operator from `candidates` at position `i`.
fn match_operator(chars: &[char], i: usize, candidates: &[&'static str]) -> Option<&'static str> {
    candidates
        .iter()
        .filter(|op| starts_with_at(chars, i, op))
        .max_by_key(|op| op.len())
        .copied()
}

/// Does `chars` contain `needle` starting at `i`?
///
/// Compares char by char rather than collecting `needle` into a `Vec`. This is
/// called once per candidate per character position — fifteen times for every
/// character of the command — so an allocation here is an allocation per
/// character, which is what it used to be.
fn starts_with_at(chars: &[char], i: usize, needle: &str) -> bool {
    (i..)
        .zip(needle.chars())
        .all(|(offset, c)| chars.get(offset) == Some(&c))
}

fn find_char(chars: &[char], from: usize, target: char) -> Option<usize> {
    (from..chars.len()).find(|&i| chars[i] == target)
}

/// Find the closing double quote, honouring backslash escapes.
fn find_double_quote_end(chars: &[char], from: usize) -> Option<usize> {
    let mut i = from;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            '"' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// Find the `)` matching a `$(` that opened just before `from`.
fn find_closing_paren(chars: &[char], from: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut i = from;
    while i < chars.len() {
        match chars[i] {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn contains_substitution(s: &str) -> bool {
    s.contains("$(")
        || s.contains('`')
        || s.contains("<(")
        || s.contains(">(")
        || s.contains("=(")
}

/// `=(cmd)` is Zsh process substitution only at word start (not `VAR=value`).
fn is_process_sub_equals_context(chars: &[char], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let prev = chars[i - 1];
    prev.is_whitespace() || matches!(prev, ';' | '|' | '&' | '(' | ')')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(input: &str) -> Vec<Vec<String>> {
        tokenize(input)
            .segments
            .iter()
            .map(|s| s.words.iter().map(|w| w.text.clone()).collect())
            .collect()
    }

    #[test]
    fn splits_plain_command() {
        assert_eq!(words("ls -la /tmp"), vec![vec!["ls", "-la", "/tmp"]]);
    }

    #[test]
    fn splits_on_separators() {
        assert_eq!(words("a && b"), vec![vec!["a"], vec!["b"]]);
        assert_eq!(words("a; b"), vec![vec!["a"], vec!["b"]]);
        assert_eq!(words("a || b"), vec![vec!["a"], vec!["b"]]);
        assert_eq!(words("a | b"), vec![vec!["a"], vec!["b"]]);
        assert_eq!(words("a\nb"), vec![vec!["a"], vec!["b"]]);
    }

    #[test]
    fn strips_quotes_but_keeps_content_as_one_word() {
        assert_eq!(words(r#"rm "my file""#), vec![vec!["rm", "my file"]]);
        assert_eq!(words("rm 'my file'"), vec![vec!["rm", "my file"]]);
    }

    #[test]
    fn keeps_variables_unexpanded() {
        assert_eq!(words("rm -rf $HOME"), vec![vec!["rm", "-rf", "$HOME"]]);
        assert_eq!(words(r#"rm -rf "$HOME""#), vec![vec!["rm", "-rf", "$HOME"]]);
    }

    #[test]
    fn separators_inside_quotes_do_not_split() {
        assert_eq!(words(r#"echo "a && b""#), vec![vec!["echo", "a && b"]]);
        assert_eq!(words("echo 'a; b'"), vec![vec!["echo", "a; b"]]);
    }

    #[test]
    fn marks_command_substitution_dynamic() {
        let t = tokenize("rm -rf $(cat list)");
        assert!(t.segments[0].has_dynamic());
        let t = tokenize("rm -rf `cat list`");
        assert!(t.segments[0].has_dynamic());
        let t = tokenize(r#"rm -rf "$(cat list)""#);
        assert!(t.segments[0].has_dynamic());
    }

    #[test]
    fn marks_process_substitution_dynamic() {
        // Bash process substitution feeds a pipe path from a nested command.
        assert!(tokenize("diff <(sort a) <(sort b)").segments[0].has_dynamic());
        assert!(tokenize("cmd >(tee log)").segments[0].has_dynamic());
        // Zsh =(cmd) at word start.
        assert!(tokenize("=(echo hi)").segments[0].has_dynamic());
        // VAR=value assignment is not process substitution.
        assert!(!tokenize("FOO=bar ls").segments[0].has_dynamic());
    }

    #[test]
    fn plain_command_is_not_dynamic() {
        assert!(!tokenize("rm -rf build").segments[0].has_dynamic());
        assert!(!tokenize("rm -rf $HOME").segments[0].has_dynamic());
    }

    #[test]
    fn unbalanced_quote_is_malformed() {
        assert!(tokenize(r#"rm "unclosed"#).malformed);
        assert!(tokenize("rm 'unclosed").malformed);
        assert!(!tokenize("rm closed").malformed);
    }

    #[test]
    fn splits_redirects_into_their_own_words() {
        assert_eq!(
            words("echo hi > out.txt"),
            vec![vec!["echo", "hi", ">", "out.txt"]]
        );
        assert_eq!(
            words("echo hi >out.txt"),
            vec![vec!["echo", "hi", ">", "out.txt"]]
        );
        assert_eq!(
            words("echo hi >> out.txt"),
            vec![vec!["echo", "hi", ">>", "out.txt"]]
        );
    }

    #[test]
    fn longest_operator_wins() {
        assert_eq!(words("a >> b"), vec![vec!["a", ">>", "b"]]);
        assert_eq!(words("a && b"), vec![vec!["a"], vec!["b"]]);
    }

    #[test]
    fn command_skips_assignments_and_sudo() {
        assert_eq!(tokenize("FOO=1 rm x").segments[0].command(), Some("rm"));
        assert_eq!(tokenize("sudo rm x").segments[0].command(), Some("rm"));
        assert_eq!(
            tokenize("FOO=1 sudo rm x").segments[0].command(),
            Some("rm")
        );
        assert_eq!(tokenize("ls").segments[0].command(), Some("ls"));
    }

    #[test]
    fn args_excludes_the_command_name() {
        let t = tokenize("rm -rf build");
        let args: Vec<&str> = t.segments[0].args().map(|w| w.text.as_str()).collect();
        assert_eq!(args, vec!["-rf", "build"]);
    }

    #[test]
    fn backslash_escape_keeps_next_char_literal() {
        assert_eq!(words(r"rm a\ b"), vec![vec!["rm", "a b"]]);
    }

    #[test]
    fn empty_input_yields_no_segments() {
        assert!(tokenize("").segments.is_empty());
        assert!(tokenize("   ").segments.is_empty());
    }
}
