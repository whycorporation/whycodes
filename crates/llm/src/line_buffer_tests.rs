use super::*;

#[test]
fn multibyte_char_split_across_chunks_decodes_intact() {
    let text = "data: {\"t\":\"şğı 你好 🙂\"}\n";
    let bytes = text.as_bytes();
    // Split inside every multi-byte sequence, one byte at a time.
    for cut in 1..bytes.len() {
        let mut lb = LineBuffer::default();
        lb.push(&bytes[..cut]);
        let mut lines: Vec<String> = std::iter::from_fn(|| lb.next_line()).collect();
        lb.push(&bytes[cut..]);
        lines.extend(std::iter::from_fn(|| lb.next_line()));
        assert_eq!(lines, vec![text.trim().to_string()], "cut at {cut}");
    }
}

#[test]
fn yields_each_complete_line_and_keeps_the_partial_tail() {
    let mut lb = LineBuffer::default();
    lb.push(b"  a  \nb\r\nc");
    assert_eq!(lb.next_line().as_deref(), Some("a"));
    assert_eq!(lb.next_line().as_deref(), Some("b"));
    assert_eq!(lb.next_line(), None);
    lb.push(b"d\n");
    assert_eq!(lb.next_line().as_deref(), Some("cd"));
    assert_eq!(lb.next_line(), None);
}

#[test]
fn take_rest_returns_the_unterminated_tail_and_empties() {
    let mut lb = LineBuffer::default();
    lb.push(b"done\n {\"x\":1} ");
    assert_eq!(lb.next_line().as_deref(), Some("done"));
    assert_eq!(lb.take_rest(), "{\"x\":1}");
    assert_eq!(lb.take_rest(), "");
    assert_eq!(lb.next_line(), None);
}
