//! Lightweight function extractors for Rust / TypeScript / Python.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    TypeScript,
    Python,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    pub name: String,
    pub body: String,
    pub sloc: u32,
}

pub fn language_for_path(path: &Path) -> Option<Lang> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => Some(Lang::Rust),
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => Some(Lang::TypeScript),
        "py" => Some(Lang::Python),
        _ => None,
    }
}

pub fn functions(lang: Lang, src: &str) -> Vec<Function> {
    match lang {
        Lang::Rust => rust_functions(src),
        Lang::TypeScript => ts_functions(src),
        Lang::Python => py_functions(src),
    }
}

fn rust_functions(src: &str) -> Vec<Function> {
    extract_kw_functions(src, "fn")
}

fn ts_functions(src: &str) -> Vec<Function> {
    extract_kw_functions(src, "function")
}

fn extract_kw_functions(src: &str, kw: &str) -> Vec<Function> {
    let mut out = Vec::new();
    let mut search = 0;
    while let Some(rel) = src[search..].find(kw) {
        let i = search + rel;
        if let Some(name) = match_fn_name(src, i, kw)
            && let Some((body, end)) = brace_body(src, i + kw.len())
        {
            out.push(make_fn(name, body));
            search = end;
            continue;
        }
        search = i + kw.len();
    }
    out
}

fn py_functions(src: &str) -> Vec<Function> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if let Some(rest) = trimmed.strip_prefix("def ") {
            let indent = lines[i].len() - trimmed.len();
            let name = rest.split('(').next().unwrap_or("fn").trim().to_string();
            let name = if name.is_empty() { "fn".into() } else { name };
            let mut body_lines = Vec::new();
            i += 1;
            while i < lines.len() {
                let l = lines[i];
                if l.trim().is_empty() {
                    body_lines.push(l);
                    i += 1;
                    continue;
                }
                let ind = l.len() - l.trim_start().len();
                if ind <= indent {
                    break;
                }
                body_lines.push(l);
                i += 1;
            }
            let body = body_lines.join("\n");
            out.push(make_fn(name, &body));
            continue;
        }
        i += 1;
    }
    out
}

fn make_fn(name: String, body: &str) -> Function {
    Function {
        sloc: sloc(body),
        name,
        body: body.to_string(),
    }
}

pub fn sloc(src: &str) -> u32 {
    src.lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with("//") && !t.starts_with('#')
        })
        .count() as u32
}

fn match_fn_name(src: &str, i: usize, kw: &str) -> Option<String> {
    if !src[i..].starts_with(kw) {
        return None;
    }
    if i > 0 {
        let prev = src.as_bytes()[i - 1];
        if prev.is_ascii_alphanumeric() || prev == b'_' {
            return None;
        }
    }
    let after = i + kw.len();
    if after < src.len() {
        let next = src.as_bytes()[after];
        if next.is_ascii_alphanumeric() || next == b'_' {
            return None;
        }
    }
    let rest = src[after..].trim_start();
    let rest = strip_async_kw(rest);
    let ident: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if ident.is_empty() {
        Some("fn".into())
    } else {
        Some(ident)
    }
}

fn strip_async_kw(rest: &str) -> &str {
    let Some(after) = rest.strip_prefix("async") else {
        return rest;
    };
    if after
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        rest
    } else {
        after.trim_start()
    }
}

fn brace_body(src: &str, from: usize) -> Option<(&str, usize)> {
    let bytes = src.as_bytes();
    let mut i = from;
    while i < bytes.len() && bytes[i] != b'{' {
        if bytes[i] == b';' {
            return None;
        }
        i += 1;
        if i - from > 400 {
            return None;
        }
    }
    if i >= bytes.len() {
        return None;
    }
    let start = i;
    let mut depth = 0i32;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    let end = i + 1;
                    return Some((&src[start..end], end));
                }
            }
            b'"' | b'\'' => {
                i = skip_string(bytes, i);
                continue;
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn skip_string(bytes: &[u8], mut i: usize) -> usize {
    let quote = bytes[i];
    i += 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i = i.saturating_add(2);
            continue;
        }
        if bytes[i] == quote {
            return i + 1;
        }
        i += 1;
    }
    i
}

#[cfg(test)]
#[path = "parse_tests.rs"]
mod tests;
