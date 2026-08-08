//! Post-turn auto-retain (Grok Hindsight / Claude auto-memory spirit).
//!
//! v1: high-precision **heuristic** extraction (no extra LLM call).
//! Optional LLM extract can feed the same [`filter_and_normalize`] path.

/// Extract durable facts from the latest user (+ optional assistant) text.
pub fn extract_heuristic(user_text: &str, assistant_text: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    for line in user_text.lines().chain(
        assistant_text.unwrap_or("").lines().take(40), // don't mine huge assistant dumps
    ) {
        if let Some(fact) = line_to_fact(line) {
            out.push(fact);
        }
    }
    // Whole-message patterns (not line-based)
    if let Some(fact) = message_to_fact(user_text)
        && !out.iter().any(|f| f.eq_ignore_ascii_case(&fact))
    {
        out.push(fact);
    }
    out
}

fn line_to_fact(line: &str) -> Option<String> {
    let t = line.trim();
    if t.len() < 12 || t.len() > 400 {
        return None;
    }
    // Skip noise
    if t.starts_with("```") || t.starts_with('#') || t.starts_with('|') {
        return None;
    }
    let lower = t.to_lowercase();

    // Explicit remember / always / prefer / never / don't use
    let triggers = [
        "remember:",
        "remember that",
        "always ",
        "never ",
        "prefer ",
        "from now on",
        "don't use",
        "do not use",
        "use pnpm",
        "use npm",
        "use yarn",
        "use cargo",
        "we use ",
        "our convention",
        "in this project",
        "for this repo",
        "make sure to",
        "please always",
    ];
    if triggers.iter().any(|p| lower.contains(p)) {
        return Some(normalize_fact(t));
    }

    // Correction patterns
    if lower.starts_with("actually ")
        || lower.starts_with("no, ")
        || lower.starts_with("no —")
        || lower.contains(" not ")
            && (lower.contains("use ") || lower.contains("always") || lower.contains("prefer"))
    {
        return Some(normalize_fact(t));
    }

    None
}

fn message_to_fact(msg: &str) -> Option<String> {
    let t = msg.trim();
    if t.len() < 16 || t.len() > 280 {
        return None;
    }
    // Single-sentence preference without newlines
    if t.contains('\n') {
        return None;
    }
    let lower = t.to_lowercase();
    if lower.starts_with("always ")
        || lower.starts_with("never ")
        || lower.starts_with("prefer ")
        || lower.starts_with("remember ")
    {
        return Some(normalize_fact(t));
    }
    None
}

fn normalize_fact(s: &str) -> String {
    let mut t = s.trim().to_string();
    // Strip leading "remember:" / "remember that"
    let lower = t.to_lowercase();
    for prefix in ["remember:", "remember that ", "please ", "hey "] {
        if lower.starts_with(prefix) {
            t = t[prefix.len()..].trim().to_string();
            break;
        }
    }
    // Collapse whitespace
    let collapsed: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
    // Cap length
    if collapsed.chars().count() > 280 {
        collapsed.chars().take(277).collect::<String>() + "..."
    } else {
        collapsed
    }
}

/// Parse optional LLM JSON/list output into facts (one per line or bullet).
pub fn parse_llm_facts(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let t = line.trim().trim_start_matches(['-', '*', '•']).trim();
        if t.is_empty() || t.eq_ignore_ascii_case("none") || t.starts_with("```") {
            continue;
        }
        if t.len() >= 12 && t.len() <= 400 {
            out.push(normalize_fact(t));
        }
    }
    out
}

/// Prompt template for optional LLM-based retain (caller runs the model).
pub fn llm_retain_prompt(user: &str, assistant: &str) -> String {
    format!(
        "Extract up to 3 durable facts worth remembering about this coding project \
         or the user's preferences. Skip one-off task details, secrets, and transient state. \
         If nothing durable, reply with exactly: NONE\n\
         Output one fact per line, no numbering.\n\n\
         USER:\n{user}\n\nASSISTANT:\n{assistant}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_always_prefer() {
        let facts = extract_heuristic("Always use pnpm, never npm in this project.", None);
        assert!(!facts.is_empty());
        assert!(facts[0].to_lowercase().contains("pnpm"));
    }

    #[test]
    fn skips_chatter() {
        let facts = extract_heuristic("ok thanks", Some("You're welcome!"));
        assert!(facts.is_empty());
    }

    #[test]
    fn parse_llm_list() {
        let raw = "- Prefer cargo check -p\n- Use fish shell\nNONE\n";
        let facts = parse_llm_facts(raw);
        assert_eq!(facts.len(), 2);
    }
}
