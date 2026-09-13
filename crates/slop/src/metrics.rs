//! Verbosity (clone ∪ heuristic flags) and erosion (CC mass).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::parse::{self, Function, Lang};
use crate::{Hotspot, SourceFile};

pub(crate) struct Scored {
    pub verbosity: f64,
    pub erosion: f64,
    pub hotspots: Vec<Hotspot>,
    pub unparsed: Vec<String>,
}

pub(crate) fn score_files(files: &[SourceFile], hotspot_n: usize) -> Scored {
    let mut unparsed = Vec::new();
    let mut parsed: Vec<(&SourceFile, Lang)> = Vec::new();
    for f in files {
        match parse::language_for_path(Path::new(&f.path)) {
            Some(lang) => parsed.push((f, lang)),
            None => unparsed.push(f.path.clone()),
        }
    }

    let loc: u32 = parsed.iter().map(|(f, _)| parse::sloc(&f.content)).sum();
    let mut flagged: HashSet<(usize, usize)> = HashSet::new();
    for (i, (f, lang)) in parsed.iter().enumerate() {
        for line in verbosity_lines(*lang, &f.content) {
            flagged.insert((i, line));
        }
        for line in clone_lines(&f.content) {
            flagged.insert((i, line));
        }
    }
    let verbosity = if loc == 0 {
        0.0
    } else {
        (flagged.len() as f64 / loc as f64).min(1.0)
    };

    let mut fns: Vec<(String, Function, u32)> = Vec::new();
    for (f, lang) in &parsed {
        for fun in parse::functions(*lang, &f.content) {
            let cc = cyclomatic(&fun.body);
            fns.push((f.path.clone(), fun, cc));
        }
    }

    let mut mass_all = 0.0f64;
    let mut mass_eroded = 0.0f64;
    let mut hot: Vec<(f64, Hotspot)> = Vec::new();
    for (path, fun, cc) in &fns {
        let mass = *cc as f64 * (fun.sloc as f64).sqrt();
        mass_all += mass;
        if *cc > 10 {
            mass_eroded += mass;
        }
        hot.push((
            mass,
            Hotspot {
                path: path.clone(),
                function: fun.name.clone(),
                cc: *cc,
                sloc: fun.sloc,
            },
        ));
    }
    hot.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    hot.truncate(hotspot_n.max(1));
    let hotspots = hot.into_iter().map(|(_, h)| h).collect();
    let erosion = if mass_all == 0.0 {
        0.0
    } else {
        mass_eroded / mass_all
    };

    Scored {
        verbosity: round3(verbosity),
        erosion: round3(erosion),
        hotspots,
        unparsed,
    }
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// McCabe: 1 + decision points. Word/operator scan, not a full CFG.
pub(crate) fn cyclomatic(body: &str) -> u32 {
    let mut cc = 1u32;
    let mut ident = String::new();
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_alphabetic() {
            ident.push(c.to_ascii_lowercase());
            continue;
        }
        bump_ident(&mut ident, &mut cc);
        match c {
            '?' => cc += 1,
            '&' if chars.next_if_eq(&'&').is_some() => cc += 1,
            '|' if chars.next_if_eq(&'|').is_some() => cc += 1,
            _ => {}
        }
    }
    bump_ident(&mut ident, &mut cc);
    let arrows = body.matches("=>").count() as u32;
    if arrows > 1 {
        cc += arrows - 1;
    }
    cc.max(1)
}

fn bump_ident(ident: &mut String, cc: &mut u32) {
    match ident.as_str() {
        "if" | "elif" | "while" | "for" | "match" | "case" | "catch" | "except" | "and" | "or" => {
            *cc += 1;
        }
        _ => {}
    }
    ident.clear();
}

fn verbosity_lines(lang: Lang, src: &str) -> Vec<usize> {
    let mut out = Vec::new();
    for (i, line) in src.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if is_trivial_wrapper(lang, t) || is_generated_looking(t) || is_pass_through(t) {
            out.push(i);
        }
    }
    out
}

fn looks_like_todo_macro(t: &str) -> bool {
    t.contains("todo!")
}

fn looks_like_unimplemented_macro(t: &str) -> bool {
    t.contains("unimplemented!")
}

fn is_trivial_wrapper(lang: Lang, t: &str) -> bool {
    match lang {
        Lang::Rust => {
            (t.starts_with("pub fn ") && t.contains("-> ") && t.contains("{ ") && t.ends_with('}'))
                || t == "Ok(())"
                || looks_like_todo_macro(t)
                || looks_like_unimplemented_macro(t)
        }
        Lang::TypeScript => {
            t.starts_with("function ")
                && t.contains("return ")
                && t.contains('{')
                && t.contains('}')
        }
        Lang::Python => t == "pass" || (t.starts_with("return ") && !t.contains('(')),
    }
}

fn is_generated_looking(t: &str) -> bool {
    t.contains("TODO: generated")
        || t.contains("AUTO-GENERATED")
        || t.starts_with("// ===")
        || t.starts_with("# ===")
}

fn is_pass_through(t: &str) -> bool {
    (t.starts_with("self.") && t.contains(" = ")) || t.contains(".clone()")
}

/// Duplicate non-trivial lines (exact, trimmed, length ≥ 12, appears ≥ 2).
fn clone_lines(src: &str) -> Vec<usize> {
    let mut counts: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, line) in src.lines().enumerate() {
        let t = line.trim();
        if t.len() < 12 {
            continue;
        }
        if t.starts_with("//") || t.starts_with('#') || t.starts_with("use ") {
            continue;
        }
        counts.entry(t).or_default().push(i);
    }
    let mut out = Vec::new();
    for idxs in counts.values() {
        if idxs.len() >= 2 {
            out.extend(idxs.iter().copied());
        }
    }
    out
}

#[cfg(test)]
#[path = "metrics_tests.rs"]
mod tests;
