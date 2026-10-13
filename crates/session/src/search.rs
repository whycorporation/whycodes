//! Full-text search over saved session transcripts.

use whycodes_core::types::{ContentBlock, Message, MessageContent};
use whycodes_storage::db::Database;

/// Most terms a query may have; extra words are ignored.
const MAX_TERMS: usize = 8;

/// One matching session for a search.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSearchHit {
    pub id: String,
    pub title: String,
    pub updated_at: String,
    pub project_path: String,
    /// Messages that matched every term.
    pub hits: usize,
    /// Excerpt around the first term in one matching message.
    pub snippet: String,
}

/// Whitespace-separated query words (deduplicated, at most [`MAX_TERMS`]).
pub fn query_terms(query: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    for word in query.split_whitespace() {
        if terms.len() == MAX_TERMS {
            break;
        }
        if !terms.iter().any(|t| t.eq_ignore_ascii_case(word)) {
            terms.push(word.to_string());
        }
    }
    terms
}

/// Saved sessions whose messages contain every word of `query`.
pub fn search_sessions(
    db: &Database,
    query: &str,
    limit: usize,
) -> crate::Result<Vec<SessionSearchHit>> {
    let terms = query_terms(query);
    let rows = db.search_sessions_text(&terms, limit)?;
    Ok(rows
        .into_iter()
        .map(|row| SessionSearchHit {
            snippet: snippet(&row.sample, &terms, 120),
            id: row.session.id,
            title: row.session.title,
            updated_at: row.session.updated_at,
            project_path: row.session.project_path,
            hits: row.hits,
        })
        .collect())
}

/// Readable text of a stored message (falls back to the raw content).
fn message_text(content_json: &str) -> String {
    let Ok(msg) = serde_json::from_str::<Message>(content_json) else {
        return content_json.to_string();
    };
    match msg.content {
        MessageContent::Text(text) => text,
        MessageContent::Blocks(blocks) => blocks
            .into_iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text),
                ContentBlock::ToolResult { content, .. } => Some(content),
                ContentBlock::ToolUse { input, .. } => Some(input.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// About `width` characters around the first term, whitespace collapsed.
pub fn snippet(content_json: &str, terms: &[String], width: usize) -> String {
    let text = message_text(content_json)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars.iter().map(|c| c.to_ascii_lowercase()).collect();
    let at = terms
        .iter()
        .filter_map(|t| {
            let needle: Vec<char> = t.chars().map(|c| c.to_ascii_lowercase()).collect();
            lower
                .windows(needle.len().max(1))
                .position(|w| w == needle.as_slice())
        })
        .min()
        .unwrap_or(0);
    let start = at.saturating_sub(width / 3);
    let end = (start + width).min(chars.len());
    let mut out: String = chars[start..end].iter().collect();
    if start > 0 {
        out.insert(0, '…');
    }
    if end < chars.len() {
        out.push('…');
    }
    out
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
