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

    #[test]
    fn covers_noise_corrections_prefixes_and_prompt() {
        assert!(extract_heuristic("```remember this code fence", None).is_empty());
        assert!(extract_heuristic("# heading is not a fact", None).is_empty());
        assert!(extract_heuristic("| table row is skipped", None).is_empty());

        let corrections = extract_heuristic(
            "actually we ship from main only now\nno, do not use npm here\nno — prefer cargo test\nplease do not use left-pad ever",
            None,
        );
        assert!(corrections.iter().any(|f| f.contains("actually")));
        assert!(corrections.iter().any(|f| f.contains("npm")));
        assert!(corrections.iter().any(|f| f.contains("prefer cargo")));
        assert!(corrections.iter().any(|f| f.contains("left-pad")));
        let not_use = extract_heuristic("this is not the use case we want going forward", None);
        assert!(not_use.iter().any(|f| f.contains("not the use")));

        let from_assistant = extract_heuristic(
            "thanks",
            Some("Remember that we use pnpm in this project for installs."),
        );
        assert!(
            from_assistant
                .iter()
                .any(|f| f.to_lowercase().contains("pnpm"))
        );

        assert!(
            extract_heuristic("always go left\nand also this second line", None)
                .iter()
                .any(|f| f.contains("always go left"))
        );
        assert_eq!(
            extract_heuristic("never commit secrets to git.", None)[0],
            "never commit secrets to git."
        );
        assert_eq!(
            extract_heuristic("prefer rustfmt over manual wrapping.", None)[0],
            "prefer rustfmt over manual wrapping."
        );
        let remembered = extract_heuristic("remember we use conventional commits here", None);
        assert_eq!(remembered.len(), 1);
        assert!(remembered[0].to_lowercase().contains("conventional"));

        let only_message = extract_heuristic("remember something unique here!!", None);
        assert_eq!(only_message.len(), 1);
        assert!(only_message[0].to_lowercase().contains("unique"));
        let hey = extract_heuristic("hey always use local memory tests here", None);
        assert!(hey.iter().any(|f| f.starts_with("always use local")));

        let stripped = extract_heuristic("Remember: always run cargo test locally first", None);
        assert!(stripped.iter().any(|f| f.starts_with("always run cargo")));
        let long = format!("always {}", "x".repeat(300));
        let capped = extract_heuristic(&long, None);
        assert_eq!(capped[0].chars().count(), 280);
        assert!(capped[0].ends_with("..."));

        let prompt = llm_retain_prompt("user note", "assistant note");
        assert!(prompt.contains("USER:\nuser note"));
        assert!(prompt.contains("ASSISTANT:\nassistant note"));
    }

    #[test]
    fn parse_llm_skips_fences_and_short_lines() {
        let facts = parse_llm_facts("```\n* remember that cargo fmt is required\nshort\n");
        assert_eq!(facts, vec!["cargo fmt is required"]);
    }
}
