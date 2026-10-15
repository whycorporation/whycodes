use super::*;

/// Feed `chunks`, then finish; return (thinking, text).
fn run(chunks: &[&str]) -> (String, String) {
    let mut tags = ThinkTags::default();
    let mut events: Vec<StreamEvent> = chunks.iter().flat_map(|c| tags.push(c)).collect();
    events.extend(tags.finish());
    let (mut thinking, mut text) = (String::new(), String::new());
    for ev in events {
        match ev {
            StreamEvent::ThinkingDelta { text: t } => thinking.push_str(&t),
            StreamEvent::TextDelta { text: t } => text.push_str(&t),
            other => panic!("unexpected {other:?}"),
        }
    }
    (thinking, text)
}

#[test]
fn plain_text_passes_through() {
    assert_eq!(
        run(&["Hello", " world"]),
        (String::new(), "Hello world".into())
    );
    // A short reply that only looks like the start of a tag is still text.
    assert_eq!(run(&["<th"]), (String::new(), "<th".into()));
    assert_eq!(run(&["<b>bold</b>"]), (String::new(), "<b>bold</b>".into()));
}

#[test]
fn a_leading_tag_becomes_thinking() {
    assert_eq!(
        run(&["<think>plan it</think>\n\nThe answer."]),
        ("plan it".into(), "The answer.".into())
    );
    // Leading whitespace before the tag is allowed.
    assert_eq!(run(&["\n  <think>x</think>y"]), ("x".into(), "y".into()));
}

#[test]
fn tags_split_across_chunks_are_found() {
    assert_eq!(
        run(&[
            "<",
            "thi",
            "nk>step one",
            " step two</",
            "thi",
            "nk>",
            "Done"
        ]),
        ("step one step two".into(), "Done".into())
    );
    // A `<` inside the thought that is not the closing tag is kept.
    assert_eq!(
        run(&["<think>a < b", " is true</think>ok"]),
        ("a < b is true".into(), "ok".into())
    );
}

#[test]
fn a_later_tag_is_left_in_the_text() {
    assert_eq!(
        run(&["Answer first. <think>not reasoning</think>"]),
        (
            String::new(),
            "Answer first. <think>not reasoning</think>".into()
        )
    );
}

#[test]
fn an_unclosed_tag_without_an_answer_is_also_the_answer() {
    assert_eq!(
        run(&["<think>The file prints \"hi\".", "\n"]),
        (
            "The file prints \"hi\".\n".into(),
            "The file prints \"hi\".".into()
        )
    );
    // Only whitespace inside: nothing to show.
    assert_eq!(run(&["<think>  "]), ("  ".into(), String::new()));
    // Empty response.
    assert_eq!(run(&[]), (String::new(), String::new()));
}

#[test]
fn a_closed_tag_with_nothing_after_it_is_only_thinking() {
    assert_eq!(
        run(&["<think>all of it</think>  "]),
        ("all of it".into(), String::new())
    );
}
