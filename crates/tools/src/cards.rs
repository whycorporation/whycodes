//! Result cards for retrieval output (issue #148).
//!
//! A search answer is a header with `shown N of M`, a few short cards, and a
//! `next:` line — never the stored blob. Zero hits say *none found*, not
//! *none exist*, and name the next call.

use whycodes_memory::{CodeHit, RecallHit, SearchPage, SessionHit};

/// Per-field cap so one huge fact or chunk cannot fill the window.
pub const FIELD_CAP: usize = 300;

/// Collapse whitespace to single spaces and cap at `max` chars (`…` marks a cut).
pub fn clip(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut out: String = flat.chars().take(max).collect();
    out.push('…');
    out
}

/// `scope · query "q" · shown N of M · corpus`.
pub fn header(scope: &str, query: &str, shown: usize, matched: usize, corpus: &str) -> String {
    format!(
        "{scope} · query \"{}\" · shown {shown} of {matched} · {corpus}",
        clip(query, 80)
    )
}

/// Zero-hit answer: what was searched, that this is not proof of absence,
/// and the next call to make.
pub fn none_found(scope: &str, query: &str, corpus: &str, next: &str) -> String {
    format!(
        "{scope} · query \"{}\" · none found in {corpus} (not proof none exist)\nnext: {next}",
        clip(query, 80)
    )
}

/// One card: `[n] id · rel 0.81`, a headline, and a `match:` line when a
/// later line holds a query word the headline does not already show.
pub fn card(n: usize, id: &str, score: f32, text: &str, query: &str) -> String {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let headline = lines.next().unwrap_or("");
    let mut out = format!(
        "[{n}] {id} · rel {score:.2}\n  {}\n",
        clip(headline, FIELD_CAP)
    );
    let words: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 2)
        .map(str::to_lowercase)
        .collect();
    if let Some(line) = lines.find(|l| {
        let lower = l.to_lowercase();
        words.iter().any(|w| lower.contains(w.as_str()))
    }) {
        out.push_str(&format!("  match: {}\n", clip(line, FIELD_CAP)));
    }
    out
}

fn short(id: &str) -> &str {
    &id[..8.min(id.len())]
}

/// Cards for `memory search`.
pub fn memory_cards(query: &str, page: &SearchPage<RecallHit>) -> String {
    let corpus = format!("{} facts", page.corpus);
    if page.hits.is_empty() {
        return none_found(
            "memory search",
            query,
            &corpus,
            "drop words, or `memory action=list` to browse; `write` saves a new fact",
        );
    }
    let mut out = header(
        "memory search",
        query,
        page.hits.len(),
        page.matched,
        &corpus,
    );
    out.push('\n');
    for (i, h) in page.hits.iter().enumerate() {
        out.push_str(&card(
            i + 1,
            short(&h.entry.id),
            h.score,
            &h.entry.text,
            query,
        ));
    }
    out.push_str("next: raise `limit` for more, `memory action=list`, or `delete` by id");
    out
}

/// Cards for `code_search`.
pub fn code_cards(query: &str, page: &SearchPage<CodeHit>) -> String {
    let corpus = format!("{} chunks", page.corpus);
    if page.hits.is_empty() {
        let next = if page.corpus == 0 {
            "run `memory action=index` first (or `whycodes memory index`)"
        } else {
            "drop words, or use `grep` for an exact pattern"
        };
        return none_found("code_search", query, &corpus, next);
    }
    let mut out = header("code_search", query, page.hits.len(), page.matched, &corpus);
    out.push('\n');
    for (i, h) in page.hits.iter().enumerate() {
        let e = &h.entry;
        let id = format!("{}:{}-{}", e.path, e.start_line, e.end_line);
        out.push_str(&card(i + 1, &id, h.score, &e.text, query));
    }
    out.push_str("next: `read` a path:line range for the full chunk, or raise `limit`");
    out
}

/// Cards for past-session recall.
pub fn session_cards(query: &str, page: &SearchPage<SessionHit>) -> String {
    let corpus = format!("{} turns", page.corpus);
    if page.hits.is_empty() {
        return none_found(
            "session search",
            query,
            &corpus,
            "drop words; turns are indexed only after they are retained",
        );
    }
    let mut out = header(
        "session search",
        query,
        page.hits.len(),
        page.matched,
        &corpus,
    );
    out.push('\n');
    for (i, h) in page.hits.iter().enumerate() {
        let e = &h.entry;
        let id = format!("{} turn {}", short(&e.session_id), e.turn_index);
        out.push_str(&card(i + 1, &id, h.score, &e.text, query));
    }
    out.push_str("next: raise `limit` for more, or resume the session by id");
    out
}

#[cfg(test)]
#[path = "cards_tests.rs"]
mod tests;
