use syntect::easy::HighlightLines;
use syntect::highlighting::{Style, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::{as_24_bit_terminal_escaped, LinesWithEndings};
use std::path::Path;

/// Highlight source code with ANSI terminal escape sequences.
/// If the language is unknown, returns the code unchanged.
pub fn highlight_code(code: &str, language: &str) -> String {
    let ps = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();

    let syntax = if language.is_empty() {
        None
    } else {
        ps.find_syntax_by_token(language)
            .or_else(|| ps.find_syntax_by_extension(language))
    };

    let syntax = match syntax {
        Some(s) => s,
        None => return code.to_string(),
    };

    let theme = &ts.themes["base16-ocean.dark"];
    let mut highlighter = HighlightLines::new(syntax, theme);

    let mut output = String::with_capacity(code.len() * 2);
    for line in LinesWithEndings::from(code) {
        let ranges: Vec<(Style, &str)> = highlighter.highlight_line(line, &ps).unwrap_or_default();
        let escaped = as_24_bit_terminal_escaped(&ranges[..], false);
        output.push_str(&escaped);
    }

    output
}

/// Detect a language from a file extension.
pub fn detect_language(path: &str) -> Option<&str> {
    let path = Path::new(path);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    match ext {
        "rs" => Some("rust"),
        "py" => Some("python"),
        "js" | "jsx" => Some("javascript"),
        "ts" | "tsx" => Some("typescript"),
        "html" => Some("html"),
        "css" => Some("css"),
        "json" => Some("json"),
        "toml" => Some("toml"),
        "yaml" | "yml" => Some("yaml"),
        "md" => Some("markdown"),
        "sh" | "bash" => Some("bash"),
        "sql" => Some("sql"),
        "c" => Some("c"),
        "cpp" | "cc" | "cxx" | "h" | "hpp" => Some("c++"),
        "go" => Some("go"),
        "java" => Some("java"),
        "rb" => Some("ruby"),
        "swift" => Some("swift"),
        "kt" | "kts" => Some("kotlin"),
        "scala" => Some("scala"),
        "r" => Some("r"),
        "lua" => Some("lua"),
        "php" => Some("php"),
        "xml" => Some("xml"),
        "dockerfile" | "Dockerfile" => Some("dockerfile"),
        "makefile" | "Makefile" => Some("makefile"),
        _ => None,
    }
}
