use super::*;

#[test]
fn encode_is_stable_and_ascii() {
    let a = tag_of("hello", 2);
    let b = tag_of("hello", 2);
    assert_eq!(a, b);
    assert_eq!(a.len(), 2);
    assert!(a.bytes().all(is_tag_char));
    assert_ne!(tag_of("hello", 2), tag_of("world", 2));
}

#[test]
fn longer_prefix_extends_shorter() {
    let h = hash_u64("a distinct line of source");
    let two = encode(h, 2);
    let three = encode(h, 3);
    assert!(three.starts_with(&two), "{three} should start with {two}");
}

#[test]
fn window_lengthens_on_content_collision() {
    // Two different strings that collide at len=2 are rare; force uniqueness
    // by checking the helper never maps different contents to one tag.
    let lines = ["alpha", "bravo", "alpha"];
    let tags = tags_for_window(&lines);
    assert_eq!(tags.len(), 3);
    assert_eq!(tags[0], tags[2], "identical content shares a tag");
    assert_ne!(tags[0], tags[1]);
}

#[test]
fn window_lengthens_when_short_prefixes_collide() {
    let mut first: Option<(String, String)> = None;
    let mut second = None;
    for i in 0..50_000 {
        let s = format!("line-{i}");
        let t = tag_of(&s, MIN_LEN);
        match &first {
            None => first = Some((s, t)),
            Some((a, at)) if at == &t && a != &s => {
                second = Some(s);
                break;
            }
            _ => {}
        }
    }
    let (a, short) = first.expect("hashed a candidate");
    let b = second.expect("two distinct lines sharing a MIN_LEN tag");
    let tags = tags_for_window(&[&a, &b]);
    assert_ne!(tags[0], tags[1]);
    assert!(tags[0].len() > MIN_LEN, "{} vs short {short}", tags[0]);
    assert!(tags[0].starts_with(&short) || tags[1].starts_with(&tag_of(&b, MIN_LEN)));
}

#[test]
fn find_tag_unique_missing_ambiguous() {
    let s = "one\ntwo\none\n";
    let offs = line_offsets(s);
    assert_eq!(offs.len(), 3);
    let t2 = tag_of("two", 2);
    match find_tag(s, &offs, &t2) {
        TagHit::Unique(i) => assert_eq!(i, 1),
        other => panic!("expected unique, got {other:?}"),
    }
    match find_tag(s, &offs, "zz") {
        TagHit::Missing => {}
        other => panic!("expected missing, got {other:?}"),
    }
    let t1 = tag_of("one", 2);
    match find_tag(s, &offs, &t1) {
        TagHit::Ambiguous(hits) => assert_eq!(hits, vec![0, 2]),
        other => panic!("expected ambiguous, got {other:?}"),
    }
}

#[test]
fn line_offsets_crlf_and_no_trailing_empty() {
    let s = "a\r\nb\n";
    let offs = line_offsets(s);
    assert_eq!(offs.len(), 2);
    assert_eq!(line_text(s, offs[0]), "a");
    assert_eq!(line_text(s, offs[1]), "b");
}

#[test]
fn format_read_and_grep_shapes() {
    let line = format_read_line(42, "a3", "    let x = 1;");
    assert!(line.contains("42"));
    assert!(line.contains(" a3|"));
    assert!(line.ends_with("let x = 1;"));
    let g = format_grep_line("src/a.rs", 7, "k9", "fn run()", ':');
    assert_eq!(g, "src/a.rs:7 k9:fn run()");
}

#[test]
fn encode_clamps_and_empty_window() {
    let h = hash_u64("x");
    assert_eq!(encode(h, 1).len(), MIN_LEN);
    assert_eq!(encode(h, 99).len(), MAX_LEN);
    assert!(tags_for_window(&[]).is_empty());
}

#[test]
fn find_tag_rejects_bad_tokens() {
    let s = "alpha\n";
    let offs = line_offsets(s);
    for bad in ["", "a", "ABCDEF", "a1!", "abcdefg"] {
        match find_tag(s, &offs, bad) {
            TagHit::Missing => {}
            other => panic!("expected missing for {bad:?}, got {other:?}"),
        }
    }
}

#[test]
fn line_offsets_empty_and_no_terminator() {
    assert!(line_offsets("").is_empty());
    let s = "solo";
    let offs = line_offsets(s);
    assert_eq!(offs.len(), 1);
    assert_eq!(line_text(s, offs[0]), "solo");
    assert_eq!(offs[0].line_end, s.len());
}

#[test]
fn nearby_snippet_empty_and_window() {
    assert_eq!(nearby_snippet("", &[], 0, 2), "(empty file)");
    let s = "a\nb\nc\n";
    let offs = line_offsets(s);
    let snip = nearby_snippet(s, &offs, 1, 1);
    assert!(snip.contains("|a"), "{snip}");
    assert!(snip.contains("|b"), "{snip}");
    assert!(snip.contains("|c"), "{snip}");
}
