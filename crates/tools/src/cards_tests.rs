use super::*;
use whycodes_memory::{CodeChunkRow, MemoryRow, SessionChunkRow};

fn page<H>(hits: Vec<H>, matched: usize, corpus: usize) -> SearchPage<H> {
    SearchPage {
        hits,
        matched,
        corpus,
    }
}

fn fact(id: &str, text: &str, score: f32) -> RecallHit {
    RecallHit {
        entry: MemoryRow {
            id: id.into(),
            project_key: "p".into(),
            text: text.into(),
            embedding: Vec::new(),
            source_session: None,
            created_at: "now".into(),
            last_recalled_at: None,
            recall_count: 0,
        },
        score,
    }
}

#[test]
fn clip_collapses_whitespace_and_marks_cuts() {
    assert_eq!(clip("  a \n b\tc ", 10), "a b c");
    assert_eq!(clip("abcdef", 3), "abc…");
}

#[test]
fn card_has_headline_and_a_match_line_only_when_a_later_line_matches() {
    let c = card(
        1,
        "a1b2c3d4",
        0.81,
        "Windows CI links\n\nLNK1181 on sqlite3.lib",
        "sqlite lib",
    );
    assert_eq!(
        c,
        "[1] a1b2c3d4 · rel 0.81\n  Windows CI links\n  match: LNK1181 on sqlite3.lib\n"
    );
    let plain = card(2, "id", 0.5, "one line about sqlite", "sqlite");
    assert!(!plain.contains("match:"), "{plain}");
    assert_eq!(card(3, "id", 0.1, "", "q"), "[3] id · rel 0.10\n  \n");
}

#[test]
fn memory_cards_show_n_of_m_against_matches_and_cap_long_facts() {
    let long = "x".repeat(FIELD_CAP + 50);
    let p = page(
        vec![
            fact("a1b2c3d4-uuid", "bundled sqlite", 0.81),
            fact("ffff", &long, 0.4),
        ],
        41,
        86,
    );
    let out = memory_cards("sqlite bundled", &p);
    assert!(
        out.starts_with("memory search · query \"sqlite bundled\" · shown 2 of 41 · 86 facts\n"),
        "{out}"
    );
    assert!(out.contains("[1] a1b2c3d4 · rel 0.81"), "{out}");
    assert!(
        out.contains(&format!("{}…", "x".repeat(FIELD_CAP))),
        "{out}"
    );
    assert!(!out.contains(&long), "the stored blob is never dumped");
    assert!(out.ends_with("or `delete` by id"), "{out}");

    let none = memory_cards("kubernetes", &page(Vec::new(), 0, 86));
    assert_eq!(
        none,
        "memory search · query \"kubernetes\" · none found in 86 facts (not proof none exist)\n\
         next: drop words, or `memory action=list` to browse; `write` saves a new fact"
    );
}

#[test]
fn code_cards_cite_path_ranges_and_point_at_index_when_empty() {
    let hit = CodeHit {
        entry: CodeChunkRow {
            id: "c".into(),
            project_key: "p".into(),
            path: "crates/storage/src/db.rs".into(),
            start_line: 359,
            end_line: 398,
            text: "// storage\npub fn list_memories(".into(),
            embedding: Vec::new(),
            updated_at: "now".into(),
        },
        score: 0.42,
    };
    let out = code_cards("list memories", &page(vec![hit], 5, 120));
    assert!(out.starts_with("code_search · query \"list memories\" · shown 1 of 5 · 120 chunks\n"));
    assert!(
        out.contains("[1] crates/storage/src/db.rs:359-398 · rel 0.42"),
        "{out}"
    );
    assert!(out.contains("  match: pub fn list_memories("), "{out}");
    assert!(out.ends_with("or raise `limit`"), "{out}");

    let unindexed = code_cards("x", &page(Vec::new(), 0, 0));
    assert!(
        unindexed.contains("run `memory action=index` first"),
        "{unindexed}"
    );
    let missed = code_cards("x", &page(Vec::new(), 0, 9));
    assert!(
        missed.contains("use `grep` for an exact pattern"),
        "{missed}"
    );
}

#[test]
fn session_cards_name_session_and_turn() {
    let hit = SessionHit {
        entry: SessionChunkRow {
            id: "s".into(),
            project_key: "p".into(),
            session_id: "0123456789abcdef".into(),
            turn_index: 3,
            text: "User: why is the retry loop slow\nAssistant: backoff".into(),
            embedding: Vec::new(),
            created_at: "now".into(),
        },
        score: 0.3,
    };
    let out = session_cards("retry", &page(vec![hit], 1, 7));
    assert!(out.contains("[1] 01234567 turn 3 · rel 0.30"), "{out}");
    assert!(out.ends_with("resume the session by id"), "{out}");
    let none = session_cards("retry", &page(Vec::new(), 0, 0));
    assert!(none.contains("none found in 0 turns"), "{none}");
}
